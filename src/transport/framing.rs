//! Private CLP frame encoding and nonblocking local-socket I/O.
//!
//! A frame is a four-byte big-endian payload length, one kind byte, and the
//! payload. Writes wait only for bounded transport capacity so a stalled peer
//! cannot keep a session-running loop alive indefinitely.

use std::{
    io::{self, Write},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;

use super::MAX_FRAME_BYTES;

const WRITE_RETRY_INTERVAL: Duration = Duration::from_millis(1);
const WRITE_STALL_TIMEOUT: Duration = Duration::from_secs(1);

pub(super) fn read_would_block(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }

    #[cfg(target_os = "windows")]
    {
        error.raw_os_error() == Some(232)
    }

    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

pub(super) fn write_frame<T: Serialize>(stream: &mut impl Write, message: &T) -> Result<()> {
    write_all_nonblocking(stream, &json_frame_bytes(message)?)
        .context("could not write the CLP JSON frame")?;
    Ok(())
}

#[cfg(test)]
pub(super) fn write_binary_frame(stream: &mut impl Write, payload: &[u8]) -> Result<()> {
    write_raw_frame(stream, b'B', payload)
}

#[cfg(test)]
pub(super) fn write_raw_frame(stream: &mut impl Write, kind: u8, payload: &[u8]) -> Result<()> {
    write_all_nonblocking(stream, &raw_frame_bytes(kind, payload)?)
        .context("could not write the CLP frame")?;
    Ok(())
}

pub(super) fn json_frame_bytes(message: &impl Serialize) -> Result<Vec<u8>> {
    let payload = serde_json::to_vec(message).context("could not encode the CLP JSON message")?;
    raw_frame_bytes(b'J', &payload)
}

pub(super) fn raw_frame_bytes(kind: u8, payload: &[u8]) -> Result<Vec<u8>> {
    let frame_length = payload
        .len()
        .checked_add(1)
        .context("the CLP frame is too large")?;
    if frame_length > MAX_FRAME_BYTES {
        return Err(anyhow!(
            "the CLP frame is {frame_length} bytes; the limit is {MAX_FRAME_BYTES}"
        ));
    }
    let length = u32::try_from(frame_length).context("the CLP frame is too large")?;
    let mut bytes = Vec::with_capacity(4 + frame_length);
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.push(kind);
    bytes.extend_from_slice(payload);
    Ok(bytes)
}

fn write_all_nonblocking(stream: &mut impl Write, mut bytes: &[u8]) -> io::Result<()> {
    let deadline = Instant::now() + WRITE_STALL_TIMEOUT;
    while !bytes.is_empty() {
        match stream.write(bytes) {
            Ok(0) => wait_for_write_capacity(deadline)?,
            Ok(written) => bytes = &bytes[written..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if read_would_block(&error) => {
                wait_for_write_capacity(deadline)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn wait_for_write_capacity(deadline: Instant) -> io::Result<()> {
    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "the CLP peer stopped reading output",
        ));
    };
    thread::sleep(remaining.min(WRITE_RETRY_INTERVAL));
    Ok(())
}

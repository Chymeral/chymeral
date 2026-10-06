use std::{
    env,
    error::Error,
    io::{self, Read, Write},
    thread,
    time::Duration,
};

use image::{Delay, Frame, Rgba, RgbaImage, codecs::gif::GifEncoder};
use interprocess::local_socket::{
    GenericNamespaced, ToNsName, prelude::LocalSocketStream, traits::Stream as _,
};
use serde_json::{Value, json};

const ENDPOINT_ENV: &str = "CHYMERAL_CLP_ENDPOINT";
const TOKEN_ENV: &str = "CHYMERAL_CLP_TOKEN";
const MAX_FRAME_BYTES: usize = 1024 * 1024;
const BINARY_FRAGMENT_BYTES: usize = 64 * 1024;

fn main() -> Result<(), Box<dyn Error>> {
    let endpoint = env::var(ENDPOINT_ENV)?;
    let token = env::var(TOKEN_ENV)?;
    let name = endpoint.to_ns_name::<GenericNamespaced>()?;
    let mut stream = LocalSocketStream::connect(name)?;

    write_json(
        &mut stream,
        &json!({
            "type": "hello",
            "protocol": { "major": 1, "minor": 4 },
            "token": token,
            "capabilities": ["scene", "resources", "input", "animated_images"]
        }),
    )?;
    expect_type(&read_json(&mut stream)?, "init")?;
    write_json(&mut stream, &json!({ "type": "init_complete" }))?;
    expect_type(&read_json(&mut stream)?, "start")?;
    write_json(&mut stream, &json!({ "type": "started" }))?;

    let mut client = RawClient::new(stream);
    client.subscribe_raw_input()?;
    let image = client.upload_gif(&probe_gif())?;
    let left = client.create_animated_node(&image, 72.0, 104.0)?;
    let right = client.create_animated_node(&image, 356.0, 104.0)?;

    println!("CLP v1.4 animated-image probe connected.");
    println!("Both GIF nodes share one resource and must remain in phase.");
    println!("Press h to hide/show both nodes, then q to remove them and close this session.");

    let mut visible = true;
    loop {
        let message = client.read_message()?;
        if !is_pressed_key(&message, "h") && !is_pressed_key(&message, "q") {
            continue;
        }
        if is_pressed_key(&message, "h") {
            visible = !visible;
            client.set_visible(&left, visible)?;
            client.set_visible(&right, visible)?;
            println!(
                "GIF nodes are now {}; shared playback retains its phase while hidden.",
                if visible { "visible" } else { "hidden" }
            );
        }
        if is_pressed_key(&message, "q") {
            client.remove_node(&left)?;
            client.remove_node(&right)?;
            client.destroy_animated_image(&image)?;
            client.close()?;
            println!("Animated image, nodes, and only this CLP session closed cleanly.");
            break;
        }
    }

    Ok(())
}

struct RawClient {
    stream: LocalSocketStream,
    next_request_id: u64,
}

impl RawClient {
    fn new(stream: LocalSocketStream) -> Self {
        Self {
            stream,
            next_request_id: 1,
        }
    }

    fn subscribe_raw_input(&mut self) -> Result<(), Box<dyn Error>> {
        write_json(
            &mut self.stream,
            &json!({ "type": "subscribe", "subscriptions": ["raw_input"] }),
        )?;
        loop {
            let message = self.read_message()?;
            if message["type"] == "subscribed" {
                return Ok(());
            }
        }
    }

    fn upload_gif(&mut self, bytes: &[u8]) -> Result<String, Box<dyn Error>> {
        let result = self.command(
            json!({
                "op": "upload_animated_image",
                "format": "gif",
                "byte_length": bytes.len(),
            }),
            Some(bytes),
        )?;
        result["image"]
            .as_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| "animated-image result did not include an image handle".into())
    }

    fn create_animated_node(
        &mut self,
        image: &str,
        x: f64,
        y: f64,
    ) -> Result<String, Box<dyn Error>> {
        let result = self.command(
            json!({
                "op": "create_root",
                "layer": "overlay",
                "properties": {
                    "x": x,
                    "y": y,
                    "width": 220.0,
                    "height": 180.0,
                    "visible": true,
                    "clip_children": false,
                    "z_index": 0,
                    "overlap_enabled": false,
                    "external_drop_enabled": false,
                },
                "content": {
                    "kind": "animated_image",
                    "image": image,
                    "opacity": 1.0,
                },
            }),
            None,
        )?;
        result["node"]
            .as_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| "node result did not include a node handle".into())
    }

    fn set_visible(&mut self, node: &str, visible: bool) -> Result<(), Box<dyn Error>> {
        self.command(
            json!({
                "op": "update_node",
                "node": node,
                "properties": { "visible": visible },
            }),
            None,
        )?;
        Ok(())
    }

    fn remove_node(&mut self, node: &str) -> Result<(), Box<dyn Error>> {
        self.command(json!({ "op": "remove_node", "node": node }), None)?;
        Ok(())
    }

    fn destroy_animated_image(&mut self, image: &str) -> Result<(), Box<dyn Error>> {
        self.command(
            json!({ "op": "destroy_animated_image", "image": image }),
            None,
        )?;
        Ok(())
    }

    fn close(&mut self) -> Result<(), Box<dyn Error>> {
        write_json(&mut self.stream, &json!({ "type": "close" }))?;
        loop {
            let message = self.read_message()?;
            if message["type"] == "closed" {
                return Ok(());
            }
        }
    }

    fn command(&mut self, command: Value, payload: Option<&[u8]>) -> Result<Value, Box<dyn Error>> {
        let request_id = format!("animated-probe-{}", self.next_request_id);
        self.next_request_id += 1;
        write_json(
            &mut self.stream,
            &json!({
                "type": "command",
                "request_id": request_id,
                "command": command,
            }),
        )?;
        if let Some(payload) = payload {
            for fragment in payload.chunks(BINARY_FRAGMENT_BYTES) {
                write_frame(&mut self.stream, b'B', fragment)?;
            }
        }
        loop {
            let message = self.read_message()?;
            if message["type"] == "result"
                && message["request_id"].as_str() == Some(request_id.as_str())
            {
                return Ok(message["result"].clone());
            }
            if message["type"] == "command_error"
                && message["request_id"].as_str() == Some(request_id.as_str())
            {
                return Err(format!(
                    "Chymeral rejected {request_id}: {}: {}",
                    message["code"], message["message"]
                )
                .into());
            }
        }
    }

    fn read_message(&mut self) -> Result<Value, Box<dyn Error>> {
        read_json(&mut self.stream)
    }
}

fn probe_gif() -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = GifEncoder::new(&mut bytes);
    encoder
        .set_repeat(image::codecs::gif::Repeat::Infinite)
        .expect("the in-memory GIF encoder accepts infinite repeat");
    for color in [[245, 115, 22, 255], [37, 99, 235, 255], [168, 85, 247, 255]] {
        encoder
            .encode_frame(Frame::from_parts(
                RgbaImage::from_pixel(2, 2, Rgba(color)),
                0,
                0,
                Delay::from_numer_denom_ms(180, 1),
            ))
            .expect("the in-memory GIF frame is valid");
    }
    drop(encoder);
    bytes
}

fn is_pressed_key(message: &Value, key: &str) -> bool {
    message["type"] == "event"
        && message["event"]["kind"] == "key"
        && message["event"]["state"] == "pressed"
        && message["event"]["key"]["kind"] == "character"
        && message["event"]["key"]["value"]
            .as_str()
            .is_some_and(|value| value.eq_ignore_ascii_case(key))
}

fn expect_type(message: &Value, expected: &str) -> Result<(), Box<dyn Error>> {
    if message["type"] == expected {
        return Ok(());
    }
    Err(format!("expected CLP {expected} message, received {message}").into())
}

fn write_json(stream: &mut LocalSocketStream, message: &Value) -> io::Result<()> {
    write_frame(
        stream,
        b'J',
        &serde_json::to_vec(message).expect("probe messages serialize"),
    )
}

fn write_frame(stream: &mut LocalSocketStream, kind: u8, payload: &[u8]) -> io::Result<()> {
    let length = payload
        .len()
        .checked_add(1)
        .filter(|length| *length <= MAX_FRAME_BYTES)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "CLP frame is too large"))?;
    let length = u32::try_from(length)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "CLP frame is too large"))?;
    write_all(stream, &length.to_be_bytes())?;
    write_all(stream, &[kind])?;
    write_all(stream, payload)
}

fn write_all(stream: &mut LocalSocketStream, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        match stream.write(bytes) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "CLP peer accepted no output bytes",
                ));
            }
            Ok(written) => bytes = &bytes[written..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn read_json(stream: &mut LocalSocketStream) -> Result<Value, Box<dyn Error>> {
    let mut header = [0_u8; 4];
    read_exact(stream, &mut header)?;
    let length = usize::try_from(u32::from_be_bytes(header))?;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err("invalid CLP frame length".into());
    }
    let mut frame = vec![0_u8; length];
    read_exact(stream, &mut frame)?;
    if frame[0] != b'J' {
        return Err("expected a JSON CLP frame".into());
    }
    Ok(serde_json::from_slice(&frame[1..])?)
}

fn read_exact(stream: &mut LocalSocketStream, mut bytes: &mut [u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        match stream.read(bytes) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "CLP peer disconnected",
                ));
            }
            Ok(read) => bytes = &mut bytes[read..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

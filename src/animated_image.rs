use std::{
    collections::HashSet,
    error::Error,
    fmt::{Display, Formatter},
    io::Cursor,
    time::{Duration, Instant},
};

use image::{AnimationDecoder, ImageDecoder, Limits, codecs::gif::GifDecoder};

use crate::resources::rgba8_byte_len;

pub const MAX_ANIMATED_IMAGE_ENCODED_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_ANIMATED_IMAGE_FRAMES: usize = 300;
pub const MAX_ANIMATED_IMAGE_BYTES: usize = 64 * 1024 * 1024;
pub const MIN_FRAME_DURATION: Duration = Duration::from_millis(20);
pub const MAX_FRAME_DURATION: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AnimatedImageId {
    index: usize,
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimatedImageFormat {
    Gif,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnimatedImageError {
    EncodedLimitExceeded { limit: usize },
    Decode(String),
    EmptyFrame,
    DimensionLimitExceeded { limit: u32 },
    FrameLimitExceeded { limit: usize },
    FrameBytesLimitExceeded { limit: usize },
    DecodedBytesLimitExceeded { limit: usize },
    InvalidImage(usize),
}

impl Display for AnimatedImageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EncodedLimitExceeded { limit } => {
                write!(
                    formatter,
                    "encoded animated images may not exceed {limit} bytes"
                )
            }
            Self::Decode(error) => write!(formatter, "could not decode animated image: {error}"),
            Self::EmptyFrame => formatter.write_str("animated images may not contain empty frames"),
            Self::DimensionLimitExceeded { limit } => {
                write!(
                    formatter,
                    "animated image dimensions may not exceed {limit}"
                )
            }
            Self::FrameLimitExceeded { limit } => {
                write!(formatter, "animated images may not exceed {limit} frames")
            }
            Self::FrameBytesLimitExceeded { limit } => {
                write!(
                    formatter,
                    "an animated image frame may not exceed {limit} bytes"
                )
            }
            Self::DecodedBytesLimitExceeded { limit } => {
                write!(
                    formatter,
                    "decoded animated images may not exceed {limit} bytes"
                )
            }
            Self::InvalidImage(actual) => write!(
                formatter,
                "animated image frame has an invalid RGBA8 byte length of {actual}"
            ),
        }
    }
}

impl Error for AnimatedImageError {}

#[derive(Clone, Debug)]
pub struct AnimatedImageFrame {
    pixels: Vec<u8>,
    duration: Duration,
}

impl AnimatedImageFrame {
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub const fn duration(&self) -> Duration {
        self.duration
    }
}

#[derive(Clone, Debug)]
pub struct AnimatedImage {
    width: u32,
    height: u32,
    frames: Vec<AnimatedImageFrame>,
    repetitions: Option<u32>,
    current_frame: usize,
    completed_repetitions: u32,
    started: bool,
    finished: bool,
    next_frame: Option<Instant>,
    paused_remaining: Option<Duration>,
}

impl AnimatedImage {
    pub fn decode(
        format: AnimatedImageFormat,
        encoded: Vec<u8>,
        max_dimension: u32,
    ) -> Result<Self, AnimatedImageError> {
        match format {
            AnimatedImageFormat::Gif => Self::decode_gif(encoded, max_dimension),
        }
    }

    pub fn decode_gif(encoded: Vec<u8>, max_dimension: u32) -> Result<Self, AnimatedImageError> {
        if encoded.len() > MAX_ANIMATED_IMAGE_ENCODED_BYTES {
            return Err(AnimatedImageError::EncodedLimitExceeded {
                limit: MAX_ANIMATED_IMAGE_ENCODED_BYTES,
            });
        }

        let repetitions = gif_repetitions(&encoded)?;
        let mut decoder = GifDecoder::new(Cursor::new(encoded))
            .map_err(|error| AnimatedImageError::Decode(error.to_string()))?;
        decoder
            .set_limits(gif_limits(max_dimension))
            .map_err(|error| AnimatedImageError::Decode(error.to_string()))?;
        let mut width = 0;
        let mut height = 0;
        let mut decoded_bytes = 0_usize;
        let mut frames = Vec::new();
        for frame in decoder.into_frames() {
            if frames.len() == MAX_ANIMATED_IMAGE_FRAMES {
                return Err(AnimatedImageError::FrameLimitExceeded {
                    limit: MAX_ANIMATED_IMAGE_FRAMES,
                });
            }
            let frame = frame.map_err(|error| AnimatedImageError::Decode(error.to_string()))?;
            let duration = frame_duration(frame.delay().numer_denom_ms());
            let buffer = frame.into_buffer();
            let frame_width = buffer.width();
            let frame_height = buffer.height();
            if frame_width == 0 || frame_height == 0 {
                return Err(AnimatedImageError::EmptyFrame);
            }
            if frame_width > max_dimension || frame_height > max_dimension {
                return Err(AnimatedImageError::DimensionLimitExceeded {
                    limit: max_dimension,
                });
            }
            if frames.is_empty() {
                width = frame_width;
                height = frame_height;
            } else if frame_width != width || frame_height != height {
                return Err(AnimatedImageError::Decode(
                    "GIF frames have inconsistent dimensions".to_owned(),
                ));
            }
            let pixels = buffer.into_raw();
            let expected = rgba8_byte_len(frame_width, frame_height)
                .map_err(|_| AnimatedImageError::InvalidImage(pixels.len()))?;
            if pixels.len() != expected {
                return Err(AnimatedImageError::InvalidImage(pixels.len()));
            }
            if pixels.len() > MAX_ANIMATED_IMAGE_BYTES {
                return Err(AnimatedImageError::FrameBytesLimitExceeded {
                    limit: MAX_ANIMATED_IMAGE_BYTES,
                });
            }
            decoded_bytes = decoded_bytes.checked_add(pixels.len()).ok_or(
                AnimatedImageError::DecodedBytesLimitExceeded {
                    limit: MAX_ANIMATED_IMAGE_BYTES,
                },
            )?;
            if decoded_bytes > MAX_ANIMATED_IMAGE_BYTES {
                return Err(AnimatedImageError::DecodedBytesLimitExceeded {
                    limit: MAX_ANIMATED_IMAGE_BYTES,
                });
            }
            frames.push(AnimatedImageFrame { pixels, duration });
        }

        if frames.is_empty() {
            return Err(AnimatedImageError::Decode(
                "GIF contains no frames".to_owned(),
            ));
        }

        Ok(Self {
            width,
            height,
            frames,
            repetitions,
            current_frame: 0,
            completed_repetitions: 0,
            started: false,
            finished: false,
            next_frame: None,
            paused_remaining: None,
        })
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub fn decoded_bytes(&self) -> usize {
        self.frames.iter().map(|frame| frame.pixels.len()).sum()
    }

    pub const fn current_frame_index(&self) -> usize {
        self.current_frame
    }

    pub fn current_frame(&self) -> &AnimatedImageFrame {
        &self.frames[self.current_frame]
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    fn update_playback(&mut self, now: Instant, visible: bool) -> bool {
        if !visible {
            if let Some(next_frame) = self.next_frame.take() {
                self.paused_remaining = Some(next_frame.saturating_duration_since(now));
            }
            return false;
        }

        if self.finished {
            return false;
        }
        if self.frames.len() == 1 {
            self.finished = true;
            self.next_frame = None;
            self.paused_remaining = None;
            return false;
        }
        if !self.started {
            self.started = true;
            self.next_frame = now.checked_add(self.current_frame().duration());
            return false;
        }
        if let Some(remaining) = self.paused_remaining.take() {
            self.next_frame = now.checked_add(remaining);
        }
        let Some(next_frame) = self.next_frame else {
            return false;
        };
        if now < next_frame {
            return false;
        }

        if self.current_frame + 1 < self.frames.len() {
            self.current_frame += 1;
        } else {
            if let Some(repetitions) = self.repetitions {
                if self.completed_repetitions >= repetitions {
                    self.finished = true;
                    self.next_frame = None;
                    return true;
                }
                self.completed_repetitions = self.completed_repetitions.saturating_add(1);
            }
            self.current_frame = 0;
        }
        self.next_frame = now.checked_add(self.current_frame().duration());
        true
    }

    fn next_frame_deadline(&self) -> Option<Instant> {
        self.next_frame
    }
}

#[derive(Debug)]
struct Slot {
    generation: u64,
    image: Option<AnimatedImage>,
}

#[derive(Debug, Default)]
pub struct AnimatedImageStore {
    slots: Vec<Slot>,
    free_slots: Vec<usize>,
    image_count: usize,
}

impl AnimatedImageStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, image: AnimatedImage) -> AnimatedImageId {
        self.image_count += 1;
        if let Some(index) = self.free_slots.pop() {
            let slot = &mut self.slots[index];
            slot.image = Some(image);
            AnimatedImageId {
                index,
                generation: slot.generation,
            }
        } else {
            let index = self.slots.len();
            self.slots.push(Slot {
                generation: 0,
                image: Some(image),
            });
            AnimatedImageId {
                index,
                generation: 0,
            }
        }
    }

    pub const fn image_count(&self) -> usize {
        self.image_count
    }

    pub fn image(&self, image: AnimatedImageId) -> Option<&AnimatedImage> {
        let slot = self.slots.get(image.index)?;
        (slot.generation == image.generation)
            .then_some(slot.image.as_ref())
            .flatten()
    }

    pub fn remove(&mut self, image: AnimatedImageId) -> Option<AnimatedImage> {
        let slot = self.slots.get_mut(image.index)?;
        if slot.generation != image.generation {
            return None;
        }
        let removed = slot.image.take()?;
        if slot.generation < u64::MAX {
            slot.generation += 1;
            self.free_slots.push(image.index);
        }
        self.image_count -= 1;
        Some(removed)
    }

    pub fn update_playback(&mut self, now: Instant, visible: &HashSet<AnimatedImageId>) -> bool {
        let mut changed = false;
        for (index, slot) in self.slots.iter_mut().enumerate() {
            let id = AnimatedImageId {
                index,
                generation: slot.generation,
            };
            if let Some(image) = slot.image.as_mut() {
                changed |= image.update_playback(now, visible.contains(&id));
            }
        }
        changed
    }

    pub fn next_frame_deadline(&self) -> Option<Instant> {
        self.slots
            .iter()
            .filter_map(|slot| slot.image.as_ref())
            .filter_map(AnimatedImage::next_frame_deadline)
            .min()
    }
}

fn gif_limits(max_dimension: u32) -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(max_dimension);
    limits.max_image_height = Some(max_dimension);
    limits.max_alloc = Some(MAX_ANIMATED_IMAGE_BYTES as u64);
    limits
}

fn frame_duration((numerator, denominator): (u32, u32)) -> Duration {
    if numerator == 0 || denominator == 0 {
        return MIN_FRAME_DURATION;
    }
    Duration::from_secs_f64(f64::from(numerator) / f64::from(denominator) / 1_000.0)
        .clamp(MIN_FRAME_DURATION, MAX_FRAME_DURATION)
}

fn gif_repetitions(encoded: &[u8]) -> Result<Option<u32>, AnimatedImageError> {
    const HEADER_LENGTH: usize = 6;
    const LOGICAL_SCREEN_DESCRIPTOR_LENGTH: usize = 7;
    const GLOBAL_COLOR_TABLE_FLAG: u8 = 0b1000_0000;
    const LOCAL_COLOR_TABLE_FLAG: u8 = 0b1000_0000;
    const APPLICATION_EXTENSION: u8 = 0xff;
    const IMAGE_DESCRIPTOR: u8 = 0x2c;
    const EXTENSION_INTRODUCER: u8 = 0x21;
    const TRAILER: u8 = 0x3b;
    const NETSCAPE_APPLICATION_IDENTIFIERS: [&[u8]; 2] = [b"NETSCAPE2.0", b"ANIMEXTS1.0"];

    if !matches!(encoded.get(..HEADER_LENGTH), Some(b"GIF87a" | b"GIF89a")) {
        return Err(AnimatedImageError::Decode(
            "animated image is not a GIF stream".to_owned(),
        ));
    }

    let mut position = HEADER_LENGTH;
    let logical_screen = gif_take(encoded, &mut position, LOGICAL_SCREEN_DESCRIPTOR_LENGTH)?;
    if logical_screen[4] & GLOBAL_COLOR_TABLE_FLAG != 0 {
        gif_skip_color_table(encoded, &mut position, logical_screen[4])?;
    }

    let mut repetitions = Some(0);
    while let Some(&block) = encoded.get(position) {
        position += 1;
        match block {
            TRAILER => break,
            EXTENSION_INTRODUCER => {
                let label = gif_byte(encoded, &mut position)?;
                let application_identifier = if label == APPLICATION_EXTENSION {
                    Some(gif_sub_block(encoded, &mut position)?)
                } else {
                    None
                };
                let is_loop_extension = application_identifier.is_some_and(|identifier| {
                    NETSCAPE_APPLICATION_IDENTIFIERS.contains(&identifier)
                });
                loop {
                    let sub_block = gif_sub_block(encoded, &mut position)?;
                    if sub_block.is_empty() {
                        break;
                    }
                    if is_loop_extension && sub_block.len() == 3 && sub_block[0] == 1 {
                        let count = u16::from_le_bytes([sub_block[1], sub_block[2]]);
                        repetitions = (count != 0).then_some(u32::from(count));
                    }
                }
            }
            IMAGE_DESCRIPTOR => {
                let descriptor = gif_take(encoded, &mut position, 9)?;
                if descriptor[8] & LOCAL_COLOR_TABLE_FLAG != 0 {
                    gif_skip_color_table(encoded, &mut position, descriptor[8])?;
                }
                gif_byte(encoded, &mut position)?;
                gif_skip_sub_blocks(encoded, &mut position)?;
            }
            _ => {
                return Err(AnimatedImageError::Decode(
                    "GIF contains an invalid block introducer".to_owned(),
                ));
            }
        }
    }

    Ok(repetitions)
}

fn gif_byte(encoded: &[u8], position: &mut usize) -> Result<u8, AnimatedImageError> {
    let byte = *encoded.get(*position).ok_or_else(gif_truncated)?;
    *position += 1;
    Ok(byte)
}

fn gif_sub_block<'a>(
    encoded: &'a [u8],
    position: &mut usize,
) -> Result<&'a [u8], AnimatedImageError> {
    let length = usize::from(gif_byte(encoded, position)?);
    gif_take(encoded, position, length)
}

fn gif_skip_sub_blocks(encoded: &[u8], position: &mut usize) -> Result<(), AnimatedImageError> {
    loop {
        if gif_sub_block(encoded, position)?.is_empty() {
            return Ok(());
        }
    }
}

fn gif_skip_color_table(
    encoded: &[u8],
    position: &mut usize,
    packed: u8,
) -> Result<(), AnimatedImageError> {
    let entries = 1_usize << (usize::from(packed & 0b0000_0111) + 1);
    let bytes = entries.checked_mul(3).ok_or_else(gif_truncated)?;
    gif_take(encoded, position, bytes)?;
    Ok(())
}

fn gif_take<'a>(
    encoded: &'a [u8],
    position: &mut usize,
    length: usize,
) -> Result<&'a [u8], AnimatedImageError> {
    let end = position.checked_add(length).ok_or_else(gif_truncated)?;
    let bytes = encoded.get(*position..end).ok_or_else(gif_truncated)?;
    *position = end;
    Ok(bytes)
}

fn gif_truncated() -> AnimatedImageError {
    AnimatedImageError::Decode("GIF stream ended unexpectedly".to_owned())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use image::{Delay, Frame, Rgba, RgbaImage, codecs::gif::GifEncoder};

    use super::{
        AnimatedImage, AnimatedImageFormat, AnimatedImageStore, MAX_FRAME_DURATION,
        MIN_FRAME_DURATION,
    };

    fn gif(repeat: Option<image::codecs::gif::Repeat>) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = GifEncoder::new(&mut bytes);
        if let Some(repeat) = repeat {
            encoder.set_repeat(repeat).unwrap();
        }
        for (color, delay) in [([255, 0, 0, 255], 0), ([0, 0, 255, 255], 40)] {
            encoder
                .encode_frame(Frame::from_parts(
                    RgbaImage::from_pixel(2, 2, Rgba(color)),
                    0,
                    0,
                    Delay::from_numer_denom_ms(delay, 1),
                ))
                .unwrap();
        }
        drop(encoder);
        bytes
    }

    fn gif_with_frame_count(frame_count: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = GifEncoder::new(&mut bytes);
        for _ in 0..frame_count {
            encoder
                .encode_frame(Frame::from_parts(
                    RgbaImage::from_pixel(1, 1, Rgba([255, 0, 0, 255])),
                    0,
                    0,
                    Delay::from_numer_denom_ms(20, 1),
                ))
                .unwrap();
        }
        drop(encoder);
        bytes
    }

    fn transparent_delta_gif() -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = GifEncoder::new(&mut bytes);
        encoder
            .encode_frame(Frame::from_parts(
                RgbaImage::from_pixel(2, 2, Rgba([255, 0, 0, 255])),
                0,
                0,
                Delay::from_numer_denom_ms(20, 1),
            ))
            .unwrap();
        let mut delta = RgbaImage::from_pixel(2, 2, Rgba([0, 0, 0, 0]));
        delta.put_pixel(1, 0, Rgba([0, 0, 255, 255]));
        encoder
            .encode_frame(Frame::from_parts(
                delta,
                0,
                0,
                Delay::from_numer_denom_ms(20, 1),
            ))
            .unwrap();
        drop(encoder);
        set_gif_disposal_to_keep(&mut bytes);
        bytes
    }

    fn set_gif_disposal_to_keep(bytes: &mut [u8]) {
        for offset in 0..bytes.len().saturating_sub(3) {
            if bytes[offset..].starts_with(&[0x21, 0xf9, 0x04]) {
                bytes[offset + 3] = (bytes[offset + 3] & !0x1c) | 0x04;
            }
        }
    }

    #[test]
    fn decodes_gif_with_clamped_frame_delays() {
        let image = AnimatedImage::decode(
            AnimatedImageFormat::Gif,
            gif(Some(image::codecs::gif::Repeat::Infinite)),
            64,
        )
        .unwrap();

        assert_eq!(image.frame_count(), 2);
        assert_eq!(image.current_frame().duration(), MIN_FRAME_DURATION);
        assert_eq!(image.width(), 2);
        assert_eq!(image.height(), 2);
        assert_eq!(image.decoded_bytes(), 32);
    }

    #[test]
    fn composites_transparent_delta_frames_to_the_full_canvas() {
        let image =
            AnimatedImage::decode(AnimatedImageFormat::Gif, transparent_delta_gif(), 64).unwrap();

        assert_eq!(image.frame_count(), 2);
        assert_eq!(
            image.frames[1].pixels,
            vec![
                255, 0, 0, 255, 0, 0, 255, 255, 255, 0, 0, 255, 255, 0, 0, 255,
            ]
        );
    }

    #[test]
    fn shares_one_phase_and_pauses_when_invisible() {
        let mut store = AnimatedImageStore::new();
        let image = store.insert(
            AnimatedImage::decode(
                AnimatedImageFormat::Gif,
                gif(Some(image::codecs::gif::Repeat::Infinite)),
                64,
            )
            .unwrap(),
        );
        let start = std::time::Instant::now();
        let visible = std::collections::HashSet::from([image]);

        assert!(!store.update_playback(start, &visible));
        assert!(store.update_playback(start + Duration::from_millis(21), &visible));
        assert_eq!(store.image(image).unwrap().current_frame_index(), 1);
        assert!(!store.update_playback(start + Duration::from_millis(25), &Default::default()));
        assert!(!store.update_playback(start + Duration::from_secs(5), &visible));
        assert_eq!(store.image(image).unwrap().current_frame_index(), 1);
    }

    #[test]
    fn caps_long_delays() {
        assert_eq!(super::frame_duration((20_000, 1)), MAX_FRAME_DURATION);
    }

    #[test]
    fn rejects_encoded_inputs_and_frame_counts_beyond_the_limits() {
        let oversized = vec![0; super::MAX_ANIMATED_IMAGE_ENCODED_BYTES + 1];
        assert!(matches!(
            AnimatedImage::decode(AnimatedImageFormat::Gif, oversized, 64),
            Err(super::AnimatedImageError::EncodedLimitExceeded { .. })
        ));
        assert!(matches!(
            AnimatedImage::decode(
                AnimatedImageFormat::Gif,
                gif_with_frame_count(super::MAX_ANIMATED_IMAGE_FRAMES + 1),
                64,
            ),
            Err(super::AnimatedImageError::FrameLimitExceeded { .. })
        ));
    }

    #[test]
    fn rejects_truncated_and_dimension_limited_gif_streams() {
        assert!(matches!(
            AnimatedImage::decode(AnimatedImageFormat::Gif, b"GIF89a".to_vec(), 64),
            Err(super::AnimatedImageError::Decode(_))
        ));
        assert!(AnimatedImage::decode(AnimatedImageFormat::Gif, gif(None), 1).is_err());
    }

    #[test]
    fn honors_absent_finite_and_infinite_gif_repetitions() {
        let start = std::time::Instant::now();
        let mut absent = AnimatedImage::decode(AnimatedImageFormat::Gif, gif(None), 64).unwrap();
        let mut finite = AnimatedImage::decode(
            AnimatedImageFormat::Gif,
            gif(Some(image::codecs::gif::Repeat::Finite(1))),
            64,
        )
        .unwrap();
        let mut infinite = AnimatedImage::decode(
            AnimatedImageFormat::Gif,
            gif(Some(image::codecs::gif::Repeat::Infinite)),
            64,
        )
        .unwrap();

        for image in [&mut absent, &mut finite, &mut infinite] {
            assert!(!image.update_playback(start, true));
            assert!(image.update_playback(start + Duration::from_millis(21), true));
            assert_eq!(image.current_frame_index(), 1);
        }

        assert!(absent.update_playback(start + Duration::from_millis(62), true));
        assert_eq!(absent.current_frame_index(), 1);
        assert!(finite.update_playback(start + Duration::from_millis(62), true));
        assert_eq!(finite.current_frame_index(), 0);
        assert!(infinite.update_playback(start + Duration::from_millis(62), true));
        assert_eq!(infinite.current_frame_index(), 0);

        assert!(finite.update_playback(start + Duration::from_millis(83), true));
        assert_eq!(finite.current_frame_index(), 1);
        assert!(finite.update_playback(start + Duration::from_millis(124), true));
        assert_eq!(finite.current_frame_index(), 1);
        assert!(infinite.update_playback(start + Duration::from_millis(83), true));
        assert_eq!(infinite.current_frame_index(), 1);
        assert!(infinite.update_playback(start + Duration::from_millis(124), true));
        assert_eq!(infinite.current_frame_index(), 0);
    }

    #[test]
    fn advances_at_most_one_frame_per_render_turn() {
        let mut image = AnimatedImage::decode(
            AnimatedImageFormat::Gif,
            gif(Some(image::codecs::gif::Repeat::Infinite)),
            64,
        )
        .unwrap();
        let start = std::time::Instant::now();

        assert!(!image.update_playback(start, true));
        assert!(image.update_playback(start + Duration::from_secs(1), true));
        assert_eq!(image.current_frame_index(), 1);
    }

    #[test]
    fn retains_one_resource_during_a_long_virtual_playback_run() {
        let mut store = AnimatedImageStore::new();
        let id = store.insert(
            AnimatedImage::decode(
                AnimatedImageFormat::Gif,
                gif(Some(image::codecs::gif::Repeat::Infinite)),
                64,
            )
            .unwrap(),
        );
        let visible = std::collections::HashSet::from([id]);
        let mut now = std::time::Instant::now();
        assert!(!store.update_playback(now, &visible));

        for _ in 0..100_000 {
            now = store.next_frame_deadline().unwrap();
            assert!(store.update_playback(now, &visible));
        }

        assert_eq!(store.image_count(), 1);
        assert!(store.image(id).is_some());
    }

    #[test]
    fn invalidates_removed_ids_before_reusing_animation_slots() {
        let mut store = AnimatedImageStore::new();
        let first =
            store.insert(AnimatedImage::decode(AnimatedImageFormat::Gif, gif(None), 64).unwrap());
        assert!(store.remove(first).is_some());
        let replacement =
            store.insert(AnimatedImage::decode(AnimatedImageFormat::Gif, gif(None), 64).unwrap());

        assert_ne!(first, replacement);
        assert!(store.image(first).is_none());
        assert!(store.image(replacement).is_some());
    }
}

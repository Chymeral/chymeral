use std::{
    error::Error,
    fmt::{Display, Formatter},
    sync::Arc,
};

const RGBA8_BYTES_PER_PIXEL: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureId {
    index: usize,
    generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextureError {
    EmptyDimensions,
    DimensionOverflow,
    InvalidByteLength { expected: usize, actual: usize },
}

impl Display for TextureError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyDimensions => formatter.write_str("texture dimensions must be non-zero"),
            Self::DimensionOverflow => {
                formatter.write_str("texture dimensions exceed addressable memory")
            }
            Self::InvalidByteLength { expected, actual } => write!(
                formatter,
                "RGBA8 texture requires {expected} bytes, but received {actual}"
            ),
        }
    }
}

impl Error for TextureError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texture {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    alpha: Arc<[u8]>,
}

impl Texture {
    pub fn from_rgba8(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, TextureError> {
        let expected = rgba8_byte_len(width, height)?;
        if pixels.len() != expected {
            return Err(TextureError::InvalidByteLength {
                expected,
                actual: pixels.len(),
            });
        }
        let alpha = pixels
            .iter()
            .skip(3)
            .step_by(RGBA8_BYTES_PER_PIXEL)
            .copied()
            .collect();
        Ok(Self {
            width,
            height,
            pixels,
            alpha,
        })
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub(crate) fn alpha(&self) -> Arc<[u8]> {
        Arc::clone(&self.alpha)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextureStoreError {
    InvalidTexture(TextureId),
}

impl Display for TextureStoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTexture(_) => formatter.write_str("texture handle is no longer valid"),
        }
    }
}

impl Error for TextureStoreError {}

#[derive(Debug)]
struct Slot {
    generation: u64,
    texture: Option<Texture>,
}

#[derive(Debug, Default)]
pub struct TextureStore {
    slots: Vec<Slot>,
    free_slots: Vec<usize>,
    texture_count: usize,
}

impl TextureStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.texture_count
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn insert(&mut self, texture: Texture) -> TextureId {
        self.texture_count += 1;
        if let Some(index) = self.free_slots.pop() {
            let slot = &mut self.slots[index];
            slot.texture = Some(texture);
            TextureId {
                index,
                generation: slot.generation,
            }
        } else {
            let index = self.slots.len();
            self.slots.push(Slot {
                generation: 0,
                texture: Some(texture),
            });
            TextureId {
                index,
                generation: 0,
            }
        }
    }

    pub fn texture(&self, texture: TextureId) -> Option<&Texture> {
        let slot = self.slots.get(texture.index)?;
        (slot.generation == texture.generation)
            .then_some(slot.texture.as_ref())
            .flatten()
    }

    pub fn remove(&mut self, texture: TextureId) -> Result<Texture, TextureStoreError> {
        let slot = self
            .slots
            .get_mut(texture.index)
            .ok_or(TextureStoreError::InvalidTexture(texture))?;
        if slot.generation != texture.generation {
            return Err(TextureStoreError::InvalidTexture(texture));
        }
        let removed = slot
            .texture
            .take()
            .ok_or(TextureStoreError::InvalidTexture(texture))?;
        if slot.generation < u64::MAX {
            slot.generation += 1;
            self.free_slots.push(texture.index);
        }
        self.texture_count -= 1;
        Ok(removed)
    }
}

pub fn rgba8_byte_len(width: u32, height: u32) -> Result<usize, TextureError> {
    if width == 0 || height == 0 {
        return Err(TextureError::EmptyDimensions);
    }
    let width = usize::try_from(width).map_err(|_| TextureError::DimensionOverflow)?;
    let height = usize::try_from(height).map_err(|_| TextureError::DimensionOverflow)?;
    width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(RGBA8_BYTES_PER_PIXEL))
        .ok_or(TextureError::DimensionOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texture(width: u32, height: u32, value: u8) -> Texture {
        Texture::from_rgba8(
            width,
            height,
            vec![value; width as usize * height as usize * 4],
        )
        .unwrap()
    }

    #[test]
    fn validates_rgba8_dimensions_and_byte_length() {
        assert_eq!(
            Texture::from_rgba8(0, 1, Vec::new()),
            Err(TextureError::EmptyDimensions)
        );
        assert_eq!(
            Texture::from_rgba8(u32::MAX, u32::MAX, Vec::new()),
            Err(TextureError::DimensionOverflow)
        );
        assert_eq!(
            Texture::from_rgba8(2, 1, vec![0; 7]),
            Err(TextureError::InvalidByteLength {
                expected: 8,
                actual: 7,
            })
        );
    }

    #[test]
    fn exposes_immutable_rgba8_texture_data() {
        let texture = Texture::from_rgba8(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]).unwrap();

        assert_eq!(texture.width(), 2);
        assert_eq!(texture.height(), 1);
        assert_eq!(texture.pixels(), &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn invalidates_removed_ids_before_reusing_slots() {
        let mut store = TextureStore::new();
        let first = store.insert(texture(1, 1, 1));
        let removed = store.remove(first).unwrap();
        let replacement = store.insert(texture(1, 1, 2));

        assert_eq!(removed.pixels(), &[1, 1, 1, 1]);
        assert_ne!(first, replacement);
        assert!(store.texture(first).is_none());
        assert_eq!(store.texture(replacement).unwrap().pixels(), &[2, 2, 2, 2]);
    }

    #[test]
    fn rejects_removing_an_invalid_id() {
        let mut store = TextureStore::new();
        let texture = store.insert(texture(1, 1, 0));
        store.remove(texture).unwrap();

        assert_eq!(
            store.remove(texture),
            Err(TextureStoreError::InvalidTexture(texture))
        );
    }

    #[test]
    fn tracks_live_texture_count() {
        let mut store = TextureStore::new();
        let first = store.insert(texture(1, 1, 0));
        let second = store.insert(texture(2, 1, 1));

        assert_eq!(store.len(), 2);
        store.remove(first).unwrap();
        assert_eq!(store.len(), 1);
        store.remove(second).unwrap();
        assert!(store.is_empty());
    }
}

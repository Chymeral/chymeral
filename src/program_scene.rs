//! Session-owned retained-scene state for CLP applications.
//!
//! This facade owns retained-scene state layout and re-exports its host-internal
//! model. Private sibling modules translate language-neutral CLP scene commands
//! into the renderer's scene graph while preserving opaque ownership boundaries,
//! resource quotas, and interaction state. The types here are used by the
//! transport and executable rather than a stable Rust application API.
//!
//! Scene batches are the transactional boundary: structural node mutations
//! either commit together or restore their captured scene state. Image upload,
//! animation decoding, and native window metadata deliberately remain outside
//! that boundary.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    animated_image::{AnimatedImage, AnimatedImageFormat, AnimatedImageId, AnimatedImageStore},
    protocol::SessionId,
    resources::{Texture, TextureStore, rgba8_byte_len},
    runtime::SceneEvent,
    scene::{
        AnimatedImageNode, ImageAlphaMask, ImageContactShape, ImageNode, NodeContent, NodeId,
        NodeProperties, Rect, Scene, SceneLayer, SolidNode,
    },
    scene_text::{TextLayout, TextLayoutError, TextMetrics, TextNode, TextStyle},
    window_metadata::{
        MAX_WINDOW_ICON_BYTES, MAX_WINDOW_ICON_DIMENSION, WindowMetadata, WindowMetadataClaims,
        WindowMetadataPatch,
    },
};

mod batch;
mod execution;
mod model;
mod resources;

pub use model::*;

pub const MAX_TEXTURE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_SESSION_TEXTURE_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_SESSION_OVERLAP_NODES: usize = 1024;
pub const MAX_SESSION_TEXT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_SESSION_TEXT_CELLS: usize = 1024 * 1024;
pub const MAX_SESSION_TEXT_NODES: usize = 1024;
pub const MAX_PRESENTATION_UPDATES: usize = 256;
/// Maximum number of structural operations in one atomic CLP scene batch.
pub const MAX_SCENE_BATCH_OPERATIONS: usize = 512;
/// Maximum UTF-8 byte length of the JSON command envelope for one scene batch.
pub const MAX_SCENE_BATCH_BYTES: usize = 512 * 1024;
/// Maximum UTF-8 byte length of one batch-local node alias.
pub const MAX_SCENE_BATCH_ALIAS_BYTES: usize = 128;

#[derive(Default)]
pub struct ProgramSceneResources {
    max_texture_dimension: u32,
    next_node_handle: u64,
    next_image_handle: u64,
    next_texture_handle: u64,
    next_animated_image_handle: u64,
    nodes: BTreeMap<NodeHandle, OwnedNode>,
    images: BTreeMap<ImageHandle, OwnedImage>,
    textures: BTreeMap<TextureHandle, OwnedTexture>,
    animated_images: BTreeMap<AnimatedImageHandle, OwnedAnimatedImage>,
    animation_store: AnimatedImageStore,
    session_texture_bytes: BTreeMap<SessionId, usize>,
    text_layout: TextLayout,
    overlap_pairs: BTreeSet<OverlapPair>,
    pending_overlap_events: Vec<SceneEvent>,
    window_metadata: WindowMetadataClaims,
}

fn restore_presentation_properties(
    scene: &mut Scene,
    changes: &[(NodeId, NodeProperties, NodeProperties)],
) {
    for (node, previous, _) in changes {
        let _ = scene.set_properties(*node, *previous);
    }
}

#[derive(Clone, Copy)]
struct OwnedNode {
    session: SessionId,
    node: NodeId,
}

#[derive(Clone, Copy)]
struct OwnedTexture {
    session: SessionId,
    texture: crate::resources::TextureId,
    bytes: usize,
}

#[derive(Clone, Copy)]
struct OwnedImage {
    session: SessionId,
    resource: ImageResource,
}

#[derive(Clone, Copy)]
enum ImageResource {
    Texture(TextureHandle),
    AnimatedImage(AnimatedImageHandle),
}

#[derive(Clone, Copy)]
struct OwnedAnimatedImage {
    session: SessionId,
    image: AnimatedImageId,
    bytes: usize,
}

#[derive(Default)]
struct TextUsage {
    bytes: usize,
    cells: usize,
    nodes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct OverlapPair {
    session: SessionId,
    first: NodeHandle,
    second: NodeHandle,
}

impl OverlapPair {
    fn new(session: SessionId, first: NodeHandle, second: NodeHandle) -> Self {
        debug_assert_ne!(first, second);
        let (first, second) = if first < second {
            (first, second)
        } else {
            (second, first)
        };
        Self {
            session,
            first,
            second,
        }
    }
}

impl ProgramSceneResources {
    pub fn new(max_texture_dimension: u32) -> Self {
        Self {
            max_texture_dimension,
            next_node_handle: 1,
            next_image_handle: 1,
            next_texture_handle: 1,
            next_animated_image_handle: 1,
            text_layout: TextLayout::new(16.0, 9.6, 20.0, [])
                .expect("default text metrics are valid"),
            ..Self::default()
        }
    }

    pub fn with_text_layout(mut self, text_layout: TextLayout) -> Self {
        self.text_layout = text_layout;
        self
    }

    pub fn animated_images(&self) -> &AnimatedImageStore {
        &self.animation_store
    }

    pub fn animated_images_mut(&mut self) -> &mut AnimatedImageStore {
        &mut self.animation_store
    }
}

fn node_handle_for(
    nodes: &BTreeMap<NodeHandle, OwnedNode>,
    node: NodeId,
) -> Result<NodeHandle, ProgramSceneError> {
    nodes
        .iter()
        .find_map(|(handle, owned)| (owned.node == node).then_some(*handle))
        .ok_or(ProgramSceneError::HandleExhausted)
}

#[cfg(test)]
mod tests {
    include!("program_scene/tests.rs");
}

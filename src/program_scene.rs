//! Session-owned retained-scene state for CLP applications.
//!
//! This module translates language-neutral CLP scene commands into the
//! renderer's scene graph while preserving opaque ownership boundaries,
//! resource quotas, and interaction state. Its public types are host-internal
//! representations used by the transport and executable rather than a stable
//! Rust application API.
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

    pub fn cleanup_session(
        &mut self,
        session: SessionId,
        scene: &mut Scene,
        textures: &mut TextureStore,
    ) -> bool {
        let metadata_changed = self.window_metadata.remove_session(session);
        let roots = self
            .nodes
            .values()
            .filter(|owned| owned.session == session)
            .map(|owned| owned.node)
            .filter(|node| {
                scene
                    .node(*node)
                    .is_some_and(|node| node.parent().is_none())
            })
            .collect::<Vec<_>>();
        for root in roots {
            let _ = scene.remove_subtree(root);
        }
        self.nodes
            .retain(|_, owned| scene.node(owned.node).is_some());
        let texture_handles = self
            .textures
            .iter()
            .filter_map(|(handle, owned)| (owned.session == session).then_some(*handle))
            .collect::<Vec<_>>();
        for handle in texture_handles {
            if let Some(owned) = self.textures.remove(&handle) {
                let _ = textures.remove(owned.texture);
            }
        }
        let animated_image_handles = self
            .animated_images
            .iter()
            .filter_map(|(handle, owned)| (owned.session == session).then_some(*handle))
            .collect::<Vec<_>>();
        for handle in animated_image_handles {
            if let Some(owned) = self.animated_images.remove(&handle) {
                let _ = self.animation_store.remove(owned.image);
            }
        }
        self.session_texture_bytes.remove(&session);
        self.images.retain(|_, owned| owned.session != session);
        self.discard_missing_overlap_pairs();
        metadata_changed
    }

    pub fn node_handle_for_session(&self, session: SessionId, node: NodeId) -> Option<NodeHandle> {
        self.nodes.iter().find_map(|(handle, owned)| {
            (owned.session == session && owned.node == node).then_some(*handle)
        })
    }

    pub fn session_for_node(&self, node: NodeId) -> Option<SessionId> {
        self.nodes
            .values()
            .find_map(|owned| (owned.node == node).then_some(owned.session))
    }

    pub fn external_drop_target_at(
        &self,
        scene: &Scene,
        point: crate::scene::Point,
    ) -> Result<Option<NodeId>, ProgramSceneError> {
        for node in scene.paint_order().into_iter().rev() {
            let Some(candidate) = scene.node(node) else {
                continue;
            };
            if !candidate.properties().external_drop_enabled
                || candidate.properties().input_passthrough
                || self.session_for_node(node).is_none()
            {
                continue;
            }
            if scene
                .visible_bounds(node)
                .map_err(ProgramSceneError::Scene)?
                .is_some_and(|bounds| bounds.contains(point))
                && scene
                    .node_contains_point(node, point)
                    .map_err(ProgramSceneError::Scene)?
            {
                return Ok(Some(node));
            }
        }
        Ok(None)
    }

    pub fn sync_animated_alpha_contacts(
        &mut self,
        scene: &mut Scene,
    ) -> Result<Vec<NodeId>, ProgramSceneError> {
        let nodes = self
            .nodes
            .values()
            .map(|owned| owned.node)
            .collect::<Vec<_>>();
        let mut changed = Vec::new();
        for node in nodes {
            let Some(NodeContent::AnimatedImage(animated)) =
                scene.node(node).map(|node| node.content())
            else {
                continue;
            };
            if !matches!(animated.contact_shape(), ImageContactShape::Alpha { .. }) {
                continue;
            }
            let image = self
                .animation_store
                .image(animated.image())
                .expect("an animated image cannot be removed while a scene node references it");
            let alpha_mask =
                ImageAlphaMask::new(image.width(), image.height(), image.current_frame().alpha())
                    .map_err(ProgramSceneError::Image)?;
            if animated
                .alpha_mask()
                .is_some_and(|current| current == &alpha_mask)
            {
                continue;
            }
            scene
                .set_content(
                    node,
                    NodeContent::AnimatedImage(animated.with_alpha_mask(alpha_mask)),
                )
                .expect("a registered animated scene node remains valid");
            changed.push(node);
        }
        Ok(changed)
    }

    pub fn reconcile_animated_alpha_contacts(
        &mut self,
        changed_nodes: &[NodeId],
        scene: &Scene,
    ) -> Result<(), ProgramSceneError> {
        let mut sessions = BTreeMap::<SessionId, Vec<NodeId>>::new();
        for node in changed_nodes {
            let Some(session) = self.session_for_node(*node) else {
                continue;
            };
            if scene
                .node(*node)
                .is_some_and(|node| node.properties().overlap_enabled)
            {
                let nodes = sessions.entry(session).or_default();
                if !nodes.contains(node) {
                    nodes.push(*node);
                }
            }
        }
        for (session, nodes) in sessions {
            self.reconcile_overlaps_for_nodes(session, &nodes, scene)?;
        }
        Ok(())
    }

    pub fn drain_overlap_events(&mut self) -> Vec<SceneEvent> {
        std::mem::take(&mut self.pending_overlap_events)
    }

    fn create_text_node(
        &self,
        session: SessionId,
        text: String,
        style: TextStyle,
        scene: &Scene,
        replaced: Option<&TextNode>,
    ) -> Result<(TextNode, TextMetrics), ProgramSceneError> {
        let metrics = self.measure_text(&text, &style)?;
        let usage = self.text_usage(session, scene);
        let previous_bytes = replaced.map(|text| text.text().len()).unwrap_or_default();
        let previous_cells = replaced
            .map(|text| text.metrics().cells)
            .unwrap_or_default();
        let previous_nodes = usize::from(replaced.is_some());
        let bytes = usage
            .bytes
            .checked_sub(previous_bytes)
            .and_then(|bytes| bytes.checked_add(text.len()))
            .ok_or(ProgramSceneError::SessionTextLimitExceeded {
                limit: MAX_SESSION_TEXT_BYTES,
            })?;
        let cells = usage
            .cells
            .checked_sub(previous_cells)
            .and_then(|cells| cells.checked_add(metrics.cells))
            .ok_or(ProgramSceneError::SessionTextLimitExceeded {
                limit: MAX_SESSION_TEXT_CELLS,
            })?;
        let nodes = usage
            .nodes
            .checked_sub(previous_nodes)
            .and_then(|nodes| nodes.checked_add(1))
            .ok_or(ProgramSceneError::SessionTextNodeLimitExceeded {
                limit: MAX_SESSION_TEXT_NODES,
            })?;
        if bytes > MAX_SESSION_TEXT_BYTES {
            return Err(ProgramSceneError::SessionTextLimitExceeded {
                limit: MAX_SESSION_TEXT_BYTES,
            });
        }
        if cells > MAX_SESSION_TEXT_CELLS {
            return Err(ProgramSceneError::SessionTextLimitExceeded {
                limit: MAX_SESSION_TEXT_CELLS,
            });
        }
        if nodes > MAX_SESSION_TEXT_NODES {
            return Err(ProgramSceneError::SessionTextNodeLimitExceeded {
                limit: MAX_SESSION_TEXT_NODES,
            });
        }
        Ok((TextNode::new(text, style, metrics), metrics))
    }

    fn measure_text(
        &self,
        text: &str,
        style: &TextStyle,
    ) -> Result<TextMetrics, ProgramSceneError> {
        self.text_layout
            .measure(text, style)
            .map_err(|error| match error {
                TextLayoutError::TextBytesExceeded { limit }
                | TextLayoutError::TextCellsExceeded { limit } => {
                    ProgramSceneError::TextLimitExceeded { limit }
                }
                error => ProgramSceneError::Text(error),
            })
    }

    fn text_node_for(
        &self,
        handle: NodeHandle,
        node: NodeId,
        scene: &Scene,
    ) -> Result<TextNode, ProgramSceneError> {
        match scene.node(node).map(|node| node.content()) {
            Some(NodeContent::Text(text)) => Ok(text.clone()),
            _ => Err(ProgramSceneError::TextNodeRequired(handle)),
        }
    }

    fn replace_text_node(
        &mut self,
        node: NodeId,
        text: TextNode,
        metrics: TextMetrics,
        scene: &mut Scene,
    ) -> Result<(), ProgramSceneError> {
        let mut properties = scene
            .node(node)
            .ok_or(ProgramSceneError::Scene(
                crate::scene::SceneError::InvalidNode(node),
            ))?
            .properties();
        properties.bounds = Rect::new(0.0, 0.0, metrics.width, metrics.height)
            .map_err(ProgramSceneError::Geometry)?;
        scene
            .set_properties(node, properties)
            .map_err(ProgramSceneError::Scene)?;
        scene
            .set_content(node, NodeContent::Text(text))
            .map_err(ProgramSceneError::Scene)
    }

    fn text_usage(&self, session: SessionId, scene: &Scene) -> TextUsage {
        self.nodes
            .values()
            .filter(|owned| owned.session == session)
            .filter_map(|owned| scene.node(owned.node))
            .filter_map(|node| match node.content() {
                NodeContent::Text(text) => Some(text),
                NodeContent::Empty
                | NodeContent::Solid(_)
                | NodeContent::Image(_)
                | NodeContent::AnimatedImage(_) => None,
            })
            .fold(TextUsage::default(), |mut usage, text| {
                usage.bytes += text.text().len();
                usage.cells += text.metrics().cells;
                usage.nodes += 1;
                usage
            })
    }

    fn validate_overlap_capacity(
        &self,
        session: SessionId,
        previously_enabled: bool,
        next_enabled: bool,
        scene: &Scene,
    ) -> Result<(), ProgramSceneError> {
        if !next_enabled || previously_enabled {
            return Ok(());
        }
        let enabled = self
            .nodes
            .values()
            .filter(|owned| {
                owned.session == session
                    && scene
                        .node(owned.node)
                        .is_some_and(|node| node.properties().overlap_enabled)
            })
            .count();
        if enabled >= MAX_SESSION_OVERLAP_NODES {
            return Err(ProgramSceneError::OverlapNodeLimitExceeded {
                limit: MAX_SESSION_OVERLAP_NODES,
            });
        }
        Ok(())
    }

    fn overlaps_for(
        &self,
        session: SessionId,
        source: NodeHandle,
        scene: &Scene,
    ) -> Result<Vec<NodeHandle>, ProgramSceneError> {
        let source = self.node_for(session, source)?;
        let mut overlaps = Vec::new();
        for (handle, owned) in &self.nodes {
            if owned.session != session || owned.node == source {
                continue;
            }
            let Some(candidate) = scene.node(owned.node) else {
                continue;
            };
            if !candidate.properties().overlap_enabled {
                continue;
            }
            if scene
                .nodes_overlap(source, owned.node)
                .map_err(ProgramSceneError::Scene)?
            {
                overlaps.push(*handle);
            }
        }
        Ok(overlaps)
    }

    fn reconcile_overlaps(
        &mut self,
        session: SessionId,
        scene: &Scene,
    ) -> Result<(), ProgramSceneError> {
        let current = self.overlap_pairs_for_session(session, scene)?;
        let previous = self
            .overlap_pairs
            .iter()
            .filter(|pair| pair.session == session)
            .copied()
            .collect::<BTreeSet<_>>();

        let entered = current
            .difference(&previous)
            .copied()
            .map(|pair| self.overlap_scene_event(pair, true))
            .collect::<Result<Vec<_>, _>>()?;
        let left = previous
            .difference(&current)
            .copied()
            .map(|pair| self.overlap_scene_event(pair, false))
            .collect::<Result<Vec<_>, _>>()?;

        self.overlap_pairs.retain(|pair| pair.session != session);
        self.overlap_pairs.extend(current);
        self.pending_overlap_events.extend(entered);
        self.pending_overlap_events.extend(left);
        Ok(())
    }

    fn reconcile_overlaps_for_nodes(
        &mut self,
        session: SessionId,
        changed_nodes: &[NodeId],
        scene: &Scene,
    ) -> Result<(), ProgramSceneError> {
        let colliders = self
            .nodes
            .iter()
            .filter_map(|(handle, owned)| {
                (owned.session == session
                    && scene
                        .node(owned.node)
                        .is_some_and(|node| node.properties().overlap_enabled))
                .then_some((*handle, owned.node))
            })
            .collect::<Vec<_>>();
        let changed = colliders
            .iter()
            .filter_map(|(handle, node)| changed_nodes.contains(node).then_some(*handle))
            .collect::<BTreeSet<_>>();
        if changed.is_empty() {
            return Ok(());
        }

        let previous = self
            .overlap_pairs
            .iter()
            .filter(|pair| {
                pair.session == session
                    && (changed.contains(&pair.first) || changed.contains(&pair.second))
            })
            .copied()
            .collect::<BTreeSet<_>>();
        let mut current = BTreeSet::new();
        for (first_handle, first_node) in &colliders {
            if !changed.contains(first_handle) {
                continue;
            }
            for (second_handle, second_node) in &colliders {
                if first_handle == second_handle {
                    continue;
                }
                if scene
                    .nodes_overlap(*first_node, *second_node)
                    .map_err(ProgramSceneError::Scene)?
                {
                    current.insert(OverlapPair::new(session, *first_handle, *second_handle));
                }
            }
        }

        let entered = current
            .difference(&previous)
            .copied()
            .map(|pair| self.overlap_scene_event(pair, true))
            .collect::<Result<Vec<_>, _>>()?;
        let left = previous
            .difference(&current)
            .copied()
            .map(|pair| self.overlap_scene_event(pair, false))
            .collect::<Result<Vec<_>, _>>()?;
        self.overlap_pairs.retain(|pair| {
            pair.session != session
                || (!changed.contains(&pair.first) && !changed.contains(&pair.second))
        });
        self.overlap_pairs.extend(current);
        self.pending_overlap_events.extend(entered);
        self.pending_overlap_events.extend(left);
        Ok(())
    }

    fn overlap_pairs_for_session(
        &self,
        session: SessionId,
        scene: &Scene,
    ) -> Result<BTreeSet<OverlapPair>, ProgramSceneError> {
        let nodes = self
            .nodes
            .iter()
            .filter_map(|(handle, owned)| {
                (owned.session == session
                    && scene
                        .node(owned.node)
                        .is_some_and(|node| node.properties().overlap_enabled))
                .then_some((*handle, owned.node))
            })
            .map(|(handle, node)| Ok((handle, node)))
            .collect::<Result<Vec<_>, _>>()?;

        let mut pairs = BTreeSet::new();
        for (index, (first_handle, first_node)) in nodes.iter().enumerate() {
            for (second_handle, second_node) in nodes.iter().skip(index + 1) {
                if scene
                    .nodes_overlap(*first_node, *second_node)
                    .map_err(ProgramSceneError::Scene)?
                {
                    pairs.insert(OverlapPair::new(session, *first_handle, *second_handle));
                }
            }
        }
        Ok(pairs)
    }

    fn overlap_scene_event(
        &self,
        pair: OverlapPair,
        entering: bool,
    ) -> Result<SceneEvent, ProgramSceneError> {
        let first = self.node_for(pair.session, pair.first)?;
        let second = self.node_for(pair.session, pair.second)?;
        Ok(if entering {
            SceneEvent::OverlapEnter { first, second }
        } else {
            SceneEvent::OverlapLeave { first, second }
        })
    }

    fn discard_missing_overlap_pairs(&mut self) {
        self.overlap_pairs.retain(|pair| {
            self.nodes.contains_key(&pair.first) && self.nodes.contains_key(&pair.second)
        });
    }

    fn set_content(
        &self,
        session: SessionId,
        node: NodeId,
        content: ContentInput,
        scene: &mut Scene,
        textures: &TextureStore,
    ) -> Result<(), ProgramSceneError> {
        let content = self.resolve_content(session, content, textures)?;
        scene
            .set_content(node, content)
            .map_err(ProgramSceneError::Scene)
    }

    fn resolve_clp_visual(
        &self,
        session: SessionId,
        visual: NodeVisualInput,
    ) -> Result<ContentInput, ProgramSceneError> {
        match visual {
            NodeVisualInput::Empty => Ok(ContentInput::Empty),
            NodeVisualInput::Rectangle { color } => Ok(ContentInput::Solid { color }),
            NodeVisualInput::Image {
                image,
                opacity,
                contact_shape,
            } => match self.image_for(session, image)?.resource {
                ImageResource::Texture(texture) => Ok(ContentInput::Image {
                    texture,
                    opacity,
                    contact_shape,
                }),
                ImageResource::AnimatedImage(image) => Ok(ContentInput::AnimatedImage {
                    image,
                    opacity,
                    contact_shape,
                }),
            },
        }
    }

    fn resolve_content(
        &self,
        session: SessionId,
        content: ContentInput,
        textures: &TextureStore,
    ) -> Result<NodeContent, ProgramSceneError> {
        Ok(match content {
            ContentInput::Empty => NodeContent::Empty,
            ContentInput::Solid { color } => NodeContent::Solid(SolidNode::new(color)),
            ContentInput::Image {
                texture,
                opacity,
                contact_shape,
            } => {
                let texture_id = self.texture_for(session, texture)?;
                let alpha_mask = if matches!(contact_shape, ImageContactShape::Alpha { .. }) {
                    let texture = textures
                        .texture(texture_id)
                        .ok_or(ProgramSceneError::InvalidTextureHandle(texture))?;
                    Some(
                        ImageAlphaMask::new(texture.width(), texture.height(), texture.alpha())
                            .map_err(ProgramSceneError::Image)?,
                    )
                } else {
                    None
                };
                NodeContent::Image(
                    ImageNode::with_contact_shape(texture_id, opacity, contact_shape, alpha_mask)
                        .map_err(ProgramSceneError::Image)?,
                )
            }
            ContentInput::AnimatedImage {
                image,
                opacity,
                contact_shape,
            } => {
                let image_id = self.animated_image_for(session, image)?;
                let alpha_mask = if matches!(contact_shape, ImageContactShape::Alpha { .. }) {
                    let image = self
                        .animation_store
                        .image(image_id)
                        .ok_or(ProgramSceneError::InvalidAnimatedImageHandle(image))?;
                    Some(
                        ImageAlphaMask::new(
                            image.width(),
                            image.height(),
                            image.current_frame().alpha(),
                        )
                        .map_err(ProgramSceneError::Image)?,
                    )
                } else {
                    None
                };
                NodeContent::AnimatedImage(
                    AnimatedImageNode::with_contact_shape(
                        image_id,
                        opacity,
                        contact_shape,
                        alpha_mask,
                    )
                    .map_err(ProgramSceneError::Image)?,
                )
            }
        })
    }

    fn node_for(
        &self,
        session: SessionId,
        handle: NodeHandle,
    ) -> Result<NodeId, ProgramSceneError> {
        let owned = self
            .nodes
            .get(&handle)
            .ok_or(ProgramSceneError::InvalidNodeHandle(handle))?;
        if owned.session != session {
            return Err(ProgramSceneError::HandleNotOwned);
        }
        Ok(owned.node)
    }

    fn texture_for(
        &self,
        session: SessionId,
        handle: TextureHandle,
    ) -> Result<crate::resources::TextureId, ProgramSceneError> {
        let owned = self
            .textures
            .get(&handle)
            .ok_or(ProgramSceneError::InvalidTextureHandle(handle))?;
        if owned.session != session {
            return Err(ProgramSceneError::HandleNotOwned);
        }
        Ok(owned.texture)
    }

    fn image_for(
        &self,
        session: SessionId,
        handle: ImageHandle,
    ) -> Result<OwnedImage, ProgramSceneError> {
        let owned = self
            .images
            .get(&handle)
            .copied()
            .ok_or(ProgramSceneError::InvalidImageHandle(handle))?;
        if owned.session != session {
            return Err(ProgramSceneError::HandleNotOwned);
        }
        Ok(owned)
    }

    fn animated_image_for(
        &self,
        session: SessionId,
        handle: AnimatedImageHandle,
    ) -> Result<AnimatedImageId, ProgramSceneError> {
        let owned = self
            .animated_images
            .get(&handle)
            .ok_or(ProgramSceneError::InvalidAnimatedImageHandle(handle))?;
        if owned.session != session {
            return Err(ProgramSceneError::HandleNotOwned);
        }
        Ok(owned.image)
    }

    fn allocate_node(
        &mut self,
        session: SessionId,
        node: NodeId,
    ) -> Result<NodeHandle, ProgramSceneError> {
        let handle = NodeHandle(self.next_node_handle);
        self.next_node_handle = self
            .next_node_handle
            .checked_add(1)
            .ok_or(ProgramSceneError::HandleExhausted)?;
        self.nodes.insert(handle, OwnedNode { session, node });
        Ok(handle)
    }

    fn allocate_image_handle(&mut self) -> Result<ImageHandle, ProgramSceneError> {
        let handle = ImageHandle(self.next_image_handle);
        self.next_image_handle = self
            .next_image_handle
            .checked_add(1)
            .ok_or(ProgramSceneError::HandleExhausted)?;
        Ok(handle)
    }

    fn allocate_texture(
        &mut self,
        session: SessionId,
        texture: crate::resources::TextureId,
        bytes: usize,
    ) -> Result<TextureHandle, ProgramSceneError> {
        let handle = TextureHandle(self.next_texture_handle);
        self.next_texture_handle = self
            .next_texture_handle
            .checked_add(1)
            .ok_or(ProgramSceneError::HandleExhausted)?;
        self.textures.insert(
            handle,
            OwnedTexture {
                session,
                texture,
                bytes,
            },
        );
        Ok(handle)
    }

    fn allocate_animated_image(&mut self) -> Result<AnimatedImageHandle, ProgramSceneError> {
        let handle = AnimatedImageHandle(self.next_animated_image_handle);
        self.next_animated_image_handle = self
            .next_animated_image_handle
            .checked_add(1)
            .ok_or(ProgramSceneError::HandleExhausted)?;
        Ok(handle)
    }

    fn reduce_session_bytes(&mut self, session: SessionId, bytes: usize) {
        if let Some(used) = self.session_texture_bytes.get_mut(&session) {
            *used = used.saturating_sub(bytes);
            if *used == 0 {
                self.session_texture_bytes.remove(&session);
            }
        }
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

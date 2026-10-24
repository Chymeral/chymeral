use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt::{Display, Formatter},
};

use crate::{
    animated_image::{
        AnimatedImage, AnimatedImageError, AnimatedImageFormat, AnimatedImageId, AnimatedImageStore,
    },
    protocol::SessionId,
    resources::{Texture, TextureError, TextureStore, rgba8_byte_len},
    runtime::SceneEvent,
    scene::{
        AnimatedImageNode, ImageAlphaMask, ImageContactShape, ImageNode, NodeContent, NodeId,
        NodeProperties, Rect, Scene, SceneLayer, SolidNode, Transform,
    },
    scene_text::{TextLayout, TextLayoutError, TextMetrics, TextNode, TextStyle},
    window_metadata::{
        MAX_WINDOW_ICON_BYTES, MAX_WINDOW_ICON_DIMENSION, WindowMetadata, WindowMetadataClaims,
        WindowMetadataError, WindowMetadataPatch,
    },
};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeHandle(u64);

impl NodeHandle {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl Display for NodeHandle {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "node-{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextureHandle(u64);

impl TextureHandle {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl Display for TextureHandle {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "texture-{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AnimatedImageHandle(u64);

impl AnimatedImageHandle {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

impl Display for AnimatedImageHandle {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "animated-image-{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ImageHandle(u64);

impl ImageHandle {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

impl Display for ImageHandle {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "image-{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodePropertiesInput {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub visible: bool,
    pub clip_children: bool,
    pub z_index: i32,
    pub overlap_enabled: bool,
    pub external_drop_enabled: bool,
    pub input_passthrough: bool,
}

impl NodePropertiesInput {
    pub fn into_scene(self) -> Result<NodeProperties, ProgramSceneError> {
        Ok(NodeProperties {
            transform: Transform::new(self.x, self.y).map_err(ProgramSceneError::Geometry)?,
            bounds: Rect::new(0.0, 0.0, self.width, self.height)
                .map_err(ProgramSceneError::Geometry)?,
            visible: self.visible,
            clip_children: self.clip_children,
            z_index: self.z_index,
            overlap_enabled: self.overlap_enabled,
            external_drop_enabled: self.external_drop_enabled,
            input_passthrough: self.input_passthrough,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextNodePropertiesInput {
    pub x: f64,
    pub y: f64,
    pub visible: bool,
    pub clip_children: bool,
    pub z_index: i32,
    pub overlap_enabled: bool,
    pub external_drop_enabled: bool,
    pub input_passthrough: bool,
}

impl TextNodePropertiesInput {
    fn into_scene(self, metrics: TextMetrics) -> Result<NodeProperties, ProgramSceneError> {
        NodePropertiesInput {
            x: self.x,
            y: self.y,
            width: metrics.width,
            height: metrics.height,
            visible: self.visible,
            clip_children: self.clip_children,
            z_index: self.z_index,
            overlap_enabled: self.overlap_enabled,
            external_drop_enabled: self.external_drop_enabled,
            input_passthrough: self.input_passthrough,
        }
        .into_scene()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NodePropertiesPatch {
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub visible: Option<bool>,
    pub clip_children: Option<bool>,
    pub z_index: Option<i32>,
    pub overlap_enabled: Option<bool>,
    pub external_drop_enabled: Option<bool>,
    pub input_passthrough: Option<bool>,
}

impl NodePropertiesPatch {
    fn apply(self, properties: NodeProperties) -> Result<NodeProperties, ProgramSceneError> {
        NodePropertiesInput {
            x: self.x.unwrap_or(properties.transform.x()),
            y: self.y.unwrap_or(properties.transform.y()),
            width: self.width.unwrap_or(properties.bounds.width()),
            height: self.height.unwrap_or(properties.bounds.height()),
            visible: self.visible.unwrap_or(properties.visible),
            clip_children: self.clip_children.unwrap_or(properties.clip_children),
            z_index: self.z_index.unwrap_or(properties.z_index),
            overlap_enabled: self.overlap_enabled.unwrap_or(properties.overlap_enabled),
            external_drop_enabled: self
                .external_drop_enabled
                .unwrap_or(properties.external_drop_enabled),
            input_passthrough: self
                .input_passthrough
                .unwrap_or(properties.input_passthrough),
        }
        .into_scene()
    }
}

/// Geometry that may be applied through CLP's frame-paced presentation lane.
///
/// Presentation deliberately excludes visibility, clipping, stacking,
/// interaction flags, content, and hierarchy. Those changes retain the
/// ordered `set_node` command semantics.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PresentationPropertiesPatch {
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
}

impl PresentationPropertiesPatch {
    pub const fn is_empty(self) -> bool {
        self.x.is_none() && self.y.is_none() && self.width.is_none() && self.height.is_none()
    }

    fn apply(self, properties: NodeProperties) -> Result<NodeProperties, ProgramSceneError> {
        if self.is_empty() {
            return Err(ProgramSceneError::InvalidPresentationUpdate(
                "a presentation update must change at least one geometry property",
            ));
        }
        NodePropertiesInput {
            x: self.x.unwrap_or(properties.transform.x()),
            y: self.y.unwrap_or(properties.transform.y()),
            width: self.width.unwrap_or(properties.bounds.width()),
            height: self.height.unwrap_or(properties.bounds.height()),
            visible: properties.visible,
            clip_children: properties.clip_children,
            z_index: properties.z_index,
            overlap_enabled: properties.overlap_enabled,
            external_drop_enabled: properties.external_drop_enabled,
            input_passthrough: properties.input_passthrough,
        }
        .into_scene()
    }
}

/// One node geometry update in a frame-paced presentation packet.
#[derive(Clone, Debug, PartialEq)]
pub struct PresentationUpdate {
    pub node: NodeHandle,
    pub properties: PresentationPropertiesPatch,
}

/// A bounded, atomic visual-state packet submitted through CLP's presentation
/// lane. The client assigns the presentation identifier so asynchronous host
/// status notifications can be correlated without a request/response round
/// trip.
#[derive(Clone, Debug, PartialEq)]
pub struct PresentationPacket {
    pub presentation_id: String,
    pub updates: Vec<PresentationUpdate>,
}

/// A reference used only while applying one atomic CLP scene batch.
///
/// Existing node handles remain session-owned and opaque. Aliases are local to
/// the batch and may only refer to a node created by an earlier operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SceneBatchNodeRef {
    Handle(NodeHandle),
    Alias(String),
}

/// A structural parent relation used by a batch node update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SceneBatchNodeParent {
    Root,
    Node(SceneBatchNodeRef),
}

/// Partial node changes accepted inside an atomic CLP scene batch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneBatchNodeChanges {
    pub properties: Option<NodePropertiesPatch>,
    pub parent: Option<SceneBatchNodeParent>,
    pub layer: Option<SceneLayer>,
    pub visual: Option<NodeVisualInput>,
    pub text: Option<String>,
    pub text_style: Option<TextStyle>,
}

/// One ordered structural operation in an atomic CLP scene batch.
#[derive(Clone, Debug, PartialEq)]
pub enum SceneBatchOperation {
    CreateNode {
        alias: String,
        parent: Option<SceneBatchNodeRef>,
        layer: Option<SceneLayer>,
        node: NodeInput,
    },
    SetNode {
        node: SceneBatchNodeRef,
        changes: SceneBatchNodeChanges,
    },
    RemoveNode {
        node: SceneBatchNodeRef,
    },
}

impl SceneBatchOperation {
    fn created_alias(&self) -> Option<&str> {
        match self {
            Self::CreateNode { alias, .. } => Some(alias),
            Self::SetNode { .. } | Self::RemoveNode { .. } => None,
        }
    }
}

/// One created node returned by a successful atomic scene batch.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneBatchCreatedNode {
    pub node: NodeHandle,
    pub text_metrics: Option<TextMetrics>,
}

/// Outcome of one ordered operation in a successful atomic scene batch.
#[derive(Clone, Debug, PartialEq)]
pub enum SceneBatchOperationResult {
    Created { alias: String },
    Updated { text_metrics: Option<TextMetrics> },
    Removed,
}

/// Result of a successful atomic CLP scene batch.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneBatchResult {
    /// Every retained batch alias and its opaque host node handle.
    pub nodes: BTreeMap<String, SceneBatchCreatedNode>,
    /// Ordered outcomes aligned with the submitted operations.
    pub operations: Vec<SceneBatchOperationResult>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ContentInput {
    Empty,
    Solid {
        color: [u8; 4],
    },
    Image {
        texture: TextureHandle,
        opacity: f32,
        contact_shape: ImageContactShape,
    },
    AnimatedImage {
        image: AnimatedImageHandle,
        opacity: f32,
        contact_shape: ImageContactShape,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeInput {
    Empty {
        properties: NodePropertiesInput,
    },
    Rectangle {
        properties: NodePropertiesInput,
        color: [u8; 4],
    },
    Image {
        properties: NodePropertiesInput,
        image: ImageHandle,
        opacity: f32,
        contact_shape: ImageContactShape,
    },
    Text {
        properties: TextNodePropertiesInput,
        text: String,
        style: TextStyle,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NodeVisualInput {
    Empty,
    Rectangle {
        color: [u8; 4],
    },
    Image {
        image: ImageHandle,
        opacity: f32,
        contact_shape: ImageContactShape,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeParent {
    Root,
    Node(NodeHandle),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeChanges {
    pub properties: Option<NodePropertiesPatch>,
    pub parent: Option<NodeParent>,
    pub layer: Option<SceneLayer>,
    pub visual: Option<NodeVisualInput>,
    pub text: Option<String>,
    pub text_style: Option<TextStyle>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ImageUpload {
    Rgba8 {
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    },
    Gif {
        bytes: Vec<u8>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowIcon {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClpSceneCommand {
    SceneBatch {
        operations: Vec<SceneBatchOperation>,
    },
    CreateNode {
        parent: Option<NodeHandle>,
        layer: Option<SceneLayer>,
        node: NodeInput,
    },
    SetNode {
        node: NodeHandle,
        changes: NodeChanges,
    },
    RemoveNode {
        node: NodeHandle,
    },
    UploadImage {
        image: ImageUpload,
    },
    RemoveImage {
        image: ImageHandle,
    },
    IsOverlapping {
        node: NodeHandle,
    },
    GetOverlaps {
        node: NodeHandle,
    },
    SetWindowMetadata {
        metadata: WindowMetadataPatch,
    },
}

impl ClpSceneCommand {
    /// Whether this command changes retained session state.
    ///
    /// A reliable mutation supersedes a packet that was still waiting in the
    /// frame-presentation mailbox. Queries intentionally observe the last
    /// committed scene and do not discard that pending visual state.
    pub const fn is_mutating(&self) -> bool {
        matches!(
            self,
            Self::SceneBatch { .. }
                | Self::CreateNode { .. }
                | Self::SetNode { .. }
                | Self::RemoveNode { .. }
                | Self::UploadImage { .. }
                | Self::RemoveImage { .. }
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClpSceneCommandResult {
    SceneBatch(SceneBatchResult),
    Node {
        node: NodeHandle,
        text_metrics: Option<TextMetrics>,
    },
    NodeUpdated {
        text_metrics: Option<TextMetrics>,
    },
    Image(ImageHandle),
    IsOverlapping(bool),
    Overlaps(Vec<NodeHandle>),
    WindowMetadata {
        icon_supported: bool,
    },
    Acknowledged,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SceneCommand {
    CreateRoot {
        layer: SceneLayer,
        properties: NodePropertiesInput,
        content: ContentInput,
    },
    CreateChild {
        parent: NodeHandle,
        properties: NodePropertiesInput,
        content: ContentInput,
    },
    UpdateNode {
        node: NodeHandle,
        properties: NodePropertiesPatch,
    },
    SetContent {
        node: NodeHandle,
        content: ContentInput,
    },
    CreateTextRoot {
        layer: SceneLayer,
        properties: TextNodePropertiesInput,
        text: String,
        style: TextStyle,
    },
    CreateTextChild {
        parent: NodeHandle,
        properties: TextNodePropertiesInput,
        text: String,
        style: TextStyle,
    },
    SetText {
        node: NodeHandle,
        text: String,
    },
    SetTextStyle {
        node: NodeHandle,
        style: TextStyle,
    },
    Reparent {
        node: NodeHandle,
        parent: Option<NodeHandle>,
    },
    RemoveNode {
        node: NodeHandle,
    },
    QueryOverlaps {
        node: NodeHandle,
    },
    UploadTexture {
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    },
    DestroyTexture {
        texture: TextureHandle,
    },
    UploadAnimatedImage {
        format: AnimatedImageFormat,
        bytes: Vec<u8>,
    },
    DestroyAnimatedImage {
        image: AnimatedImageHandle,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum SceneCommandResult {
    Node(NodeHandle),
    TextNode {
        node: NodeHandle,
        metrics: TextMetrics,
    },
    Texture(TextureHandle),
    AnimatedImage(AnimatedImageHandle),
    Overlaps(Vec<NodeHandle>),
    TextMetrics(TextMetrics),
    Acknowledged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgramSceneError {
    SceneBatchEmpty,
    SceneBatchOperationLimitExceeded {
        limit: usize,
    },
    SceneBatchAliasInvalid(&'static str),
    SceneBatchAliasDuplicate(String),
    SceneBatchAliasUnknown(String),
    SceneBatchAliasRemoved(String),
    SceneBatchOperation {
        index: usize,
        alias: Option<String>,
        source: Box<ProgramSceneError>,
    },
    InvalidNodeHandle(NodeHandle),
    InvalidImageHandle(ImageHandle),
    InvalidTextureHandle(TextureHandle),
    InvalidAnimatedImageHandle(AnimatedImageHandle),
    HandleNotOwned,
    ImageInUse(ImageHandle),
    WindowIconMustBeStaticRgba8(ImageHandle),
    WindowIconDimensionLimitExceeded {
        limit: u32,
    },
    WindowIconByteLimitExceeded {
        limit: usize,
    },
    WindowMetadata(WindowMetadataError),
    TextureInUse(TextureHandle),
    AnimatedImageInUse(AnimatedImageHandle),
    TextureDimensionLimitExceeded {
        limit: u32,
    },
    TextureLimitExceeded {
        limit: usize,
    },
    SessionTextureLimitExceeded {
        limit: usize,
    },
    Geometry(crate::scene::GeometryError),
    Scene(crate::scene::SceneError),
    Image(crate::scene::ImageNodeError),
    Texture(TextureError),
    AnimatedImage(AnimatedImageError),
    Text(TextLayoutError),
    TextNodeRequired(NodeHandle),
    IntrinsicTextBounds(NodeHandle),
    TextContentRequiresTextCommands(NodeHandle),
    NodeKindMismatch(NodeHandle),
    InvalidNodeChange(&'static str),
    TextLimitExceeded {
        limit: usize,
    },
    SessionTextLimitExceeded {
        limit: usize,
    },
    SessionTextNodeLimitExceeded {
        limit: usize,
    },
    OverlapNodeLimitExceeded {
        limit: usize,
    },
    PresentationUpdateLimitExceeded {
        limit: usize,
    },
    DuplicatePresentationNode(NodeHandle),
    InvalidPresentationUpdate(&'static str),
    HandleExhausted,
}

impl Display for ProgramSceneError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SceneBatchEmpty => {
                formatter.write_str("a scene batch must contain at least one operation")
            }
            Self::SceneBatchOperationLimitExceeded { limit } => {
                write!(
                    formatter,
                    "a scene batch may contain at most {limit} operations"
                )
            }
            Self::SceneBatchAliasInvalid(reason) => {
                write!(formatter, "a scene batch alias is invalid: {reason}")
            }
            Self::SceneBatchAliasDuplicate(alias) => {
                write!(
                    formatter,
                    "the scene batch alias {alias:?} is declared more than once"
                )
            }
            Self::SceneBatchAliasUnknown(alias) => {
                write!(
                    formatter,
                    "the scene batch alias {alias:?} has not been declared"
                )
            }
            Self::SceneBatchAliasRemoved(alias) => {
                write!(
                    formatter,
                    "the scene batch alias {alias:?} does not retain a node"
                )
            }
            Self::SceneBatchOperation {
                index,
                alias,
                source,
            } => {
                if let Some(alias) = alias {
                    write!(
                        formatter,
                        "scene batch operation {index} for alias {alias:?} failed: {source}"
                    )
                } else {
                    write!(formatter, "scene batch operation {index} failed: {source}")
                }
            }
            Self::InvalidNodeHandle(handle) => write!(formatter, "{handle} is not valid"),
            Self::InvalidImageHandle(handle) => write!(formatter, "{handle} is not valid"),
            Self::InvalidTextureHandle(handle) => write!(formatter, "{handle} is not valid"),
            Self::InvalidAnimatedImageHandle(handle) => write!(formatter, "{handle} is not valid"),
            Self::HandleNotOwned => {
                formatter.write_str("the resource handle is not owned by this session")
            }
            Self::ImageInUse(handle) => {
                write!(
                    formatter,
                    "{handle} is still referenced by a scene node or window metadata"
                )
            }
            Self::WindowIconMustBeStaticRgba8(handle) => {
                write!(
                    formatter,
                    "{handle} must reference a static RGBA8 image for window metadata"
                )
            }
            Self::WindowIconDimensionLimitExceeded { limit } => {
                write!(
                    formatter,
                    "a window icon may not exceed {limit} pixels in either dimension"
                )
            }
            Self::WindowIconByteLimitExceeded { limit } => {
                write!(formatter, "a window icon may not exceed {limit} bytes")
            }
            Self::WindowMetadata(error) => Display::fmt(error, formatter),
            Self::TextureInUse(handle) => {
                write!(formatter, "{handle} is still referenced by a scene node")
            }
            Self::AnimatedImageInUse(handle) => {
                write!(formatter, "{handle} is still referenced by a scene node")
            }
            Self::TextureDimensionLimitExceeded { limit } => {
                write!(formatter, "texture dimensions may not exceed {limit}")
            }
            Self::TextureLimitExceeded { limit } => {
                write!(formatter, "a texture may not exceed {limit} bytes")
            }
            Self::SessionTextureLimitExceeded { limit } => write!(
                formatter,
                "a session may not retain more than {limit} texture bytes"
            ),
            Self::Geometry(error) => Display::fmt(error, formatter),
            Self::Scene(error) => Display::fmt(error, formatter),
            Self::Image(error) => Display::fmt(error, formatter),
            Self::Texture(error) => Display::fmt(error, formatter),
            Self::AnimatedImage(error) => Display::fmt(error, formatter),
            Self::Text(error) => Display::fmt(error, formatter),
            Self::TextNodeRequired(handle) => write!(formatter, "{handle} does not contain text"),
            Self::IntrinsicTextBounds(handle) => {
                write!(
                    formatter,
                    "{handle} has intrinsic text bounds that cannot be set directly"
                )
            }
            Self::TextContentRequiresTextCommands(handle) => {
                write!(
                    formatter,
                    "{handle} contains text and must be changed with text commands"
                )
            }
            Self::NodeKindMismatch(handle) => {
                write!(
                    formatter,
                    "{handle} cannot change between text and visual content"
                )
            }
            Self::InvalidNodeChange(message) => formatter.write_str(message),
            Self::TextLimitExceeded { limit } => {
                write!(formatter, "text may not exceed {limit} bytes or cells")
            }
            Self::SessionTextLimitExceeded { limit } => {
                write!(
                    formatter,
                    "a session may not retain more than {limit} text bytes or cells"
                )
            }
            Self::SessionTextNodeLimitExceeded { limit } => {
                write!(
                    formatter,
                    "a session may not retain more than {limit} text nodes"
                )
            }
            Self::OverlapNodeLimitExceeded { limit } => {
                write!(
                    formatter,
                    "a session may enable overlap on at most {limit} nodes"
                )
            }
            Self::PresentationUpdateLimitExceeded { limit } => {
                write!(
                    formatter,
                    "a presentation packet may update at most {limit} nodes"
                )
            }
            Self::DuplicatePresentationNode(handle) => {
                write!(
                    formatter,
                    "a presentation packet updates {handle} more than once"
                )
            }
            Self::InvalidPresentationUpdate(message) => formatter.write_str(message),
            Self::HandleExhausted => {
                formatter.write_str("could not allocate another resource handle")
            }
        }
    }
}

impl Error for ProgramSceneError {}

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

/// The portion of retained program-scene state that structural batch commands
/// are allowed to mutate. Image uploads, animation decoding, and native window
/// metadata intentionally remain outside M26 batches, so their stores do not
/// need to be copied or rolled back here.
struct SceneBatchSnapshot {
    scene: Scene,
    next_node_handle: u64,
    nodes: BTreeMap<NodeHandle, OwnedNode>,
    overlap_pairs: BTreeSet<OverlapPair>,
    pending_overlap_events: Vec<SceneEvent>,
}

impl SceneBatchSnapshot {
    fn capture(resources: &ProgramSceneResources, scene: &Scene) -> Self {
        Self {
            scene: scene.clone(),
            next_node_handle: resources.next_node_handle,
            nodes: resources.nodes.clone(),
            overlap_pairs: resources.overlap_pairs.clone(),
            pending_overlap_events: resources.pending_overlap_events.clone(),
        }
    }

    fn restore(self, resources: &mut ProgramSceneResources, scene: &mut Scene) {
        *scene = self.scene;
        resources.next_node_handle = self.next_node_handle;
        resources.nodes = self.nodes;
        resources.overlap_pairs = self.overlap_pairs;
        resources.pending_overlap_events = self.pending_overlap_events;
    }
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

    pub fn execute_clp(
        &mut self,
        session: SessionId,
        command: ClpSceneCommand,
        scene: &mut Scene,
        textures: &mut TextureStore,
    ) -> Result<ClpSceneCommandResult, ProgramSceneError> {
        self.execute_clp_with_window_icon_support(session, command, scene, textures, false)
    }

    pub fn execute_clp_with_window_icon_support(
        &mut self,
        session: SessionId,
        command: ClpSceneCommand,
        scene: &mut Scene,
        textures: &mut TextureStore,
        window_icon_supported: bool,
    ) -> Result<ClpSceneCommandResult, ProgramSceneError> {
        match command {
            ClpSceneCommand::SceneBatch { operations } => Ok(ClpSceneCommandResult::SceneBatch(
                self.execute_scene_batch(session, operations, scene, textures)?,
            )),
            ClpSceneCommand::CreateNode {
                parent,
                layer,
                node,
            } => self.create_clp_node(session, parent, layer, node, scene, textures),
            ClpSceneCommand::SetNode { node, changes } => {
                self.set_clp_node(session, node, changes, scene, textures)
            }
            ClpSceneCommand::RemoveNode { node } => {
                self.execute(session, SceneCommand::RemoveNode { node }, scene, textures)?;
                Ok(ClpSceneCommandResult::Acknowledged)
            }
            ClpSceneCommand::UploadImage { image } => {
                let handle = self.allocate_image_handle()?;
                let resource = match image {
                    ImageUpload::Rgba8 {
                        width,
                        height,
                        pixels,
                    } => match self.execute(
                        session,
                        SceneCommand::UploadTexture {
                            width,
                            height,
                            pixels,
                        },
                        scene,
                        textures,
                    )? {
                        SceneCommandResult::Texture(texture) => ImageResource::Texture(texture),
                        _ => unreachable!("an RGBA8 upload returns a texture handle"),
                    },
                    ImageUpload::Gif { bytes } => match self.execute(
                        session,
                        SceneCommand::UploadAnimatedImage {
                            format: AnimatedImageFormat::Gif,
                            bytes,
                        },
                        scene,
                        textures,
                    )? {
                        SceneCommandResult::AnimatedImage(image) => {
                            ImageResource::AnimatedImage(image)
                        }
                        _ => unreachable!("a GIF upload returns an animated image handle"),
                    },
                };
                self.images.insert(handle, OwnedImage { session, resource });
                Ok(ClpSceneCommandResult::Image(handle))
            }
            ClpSceneCommand::RemoveImage { image } => {
                if self.window_metadata.references_image(image) {
                    return Err(ProgramSceneError::ImageInUse(image));
                }
                let owned = self.image_for(session, image)?;
                match owned.resource {
                    ImageResource::Texture(texture) => {
                        self.execute(
                            session,
                            SceneCommand::DestroyTexture { texture },
                            scene,
                            textures,
                        )
                        .map_err(|error| match error {
                            ProgramSceneError::TextureInUse(_) => {
                                ProgramSceneError::ImageInUse(image)
                            }
                            error => error,
                        })?;
                    }
                    ImageResource::AnimatedImage(animated_image) => {
                        self.execute(
                            session,
                            SceneCommand::DestroyAnimatedImage {
                                image: animated_image,
                            },
                            scene,
                            textures,
                        )
                        .map_err(|error| match error {
                            ProgramSceneError::AnimatedImageInUse(_) => {
                                ProgramSceneError::ImageInUse(image)
                            }
                            error => error,
                        })?;
                    }
                }
                self.images.remove(&image);
                Ok(ClpSceneCommandResult::Acknowledged)
            }
            ClpSceneCommand::IsOverlapping { node } => Ok(ClpSceneCommandResult::IsOverlapping(
                !self.overlaps_for(session, node, scene)?.is_empty(),
            )),
            ClpSceneCommand::GetOverlaps { node } => Ok(ClpSceneCommandResult::Overlaps(
                self.overlaps_for(session, node, scene)?,
            )),
            ClpSceneCommand::SetWindowMetadata { metadata } => {
                self.set_window_metadata(session, metadata, textures)?;
                Ok(ClpSceneCommandResult::WindowMetadata {
                    icon_supported: window_icon_supported,
                })
            }
        }
    }

    /// Applies a bounded structural command sequence without allowing an
    /// incomplete intermediate scene to escape the host event turn.
    ///
    /// Every individual operation still uses the established ownership and
    /// scene validation path. A snapshot restores the retained state and
    /// handle allocator if any operation fails. Overlap state is reset to its
    /// pre-batch snapshot and reconciled once against the final scene, so the
    /// client observes only the atomic transition rather than transient pairs.
    fn execute_scene_batch(
        &mut self,
        session: SessionId,
        operations: Vec<SceneBatchOperation>,
        scene: &mut Scene,
        textures: &mut TextureStore,
    ) -> Result<SceneBatchResult, ProgramSceneError> {
        if operations.is_empty() {
            return Err(ProgramSceneError::SceneBatchEmpty);
        }
        if operations.len() > MAX_SCENE_BATCH_OPERATIONS {
            return Err(ProgramSceneError::SceneBatchOperationLimitExceeded {
                limit: MAX_SCENE_BATCH_OPERATIONS,
            });
        }

        let operation_count = operations.len();
        let snapshot = SceneBatchSnapshot::capture(self, scene);
        let result = (|| {
            let mut aliases = BTreeMap::new();
            let mut nodes = BTreeMap::new();
            let mut results = Vec::with_capacity(operation_count);

            for (index, operation) in operations.into_iter().enumerate() {
                let alias = operation.created_alias().map(str::to_owned);
                let result = (|| -> Result<SceneBatchOperationResult, ProgramSceneError> {
                    Ok(match operation {
                        SceneBatchOperation::CreateNode {
                            alias,
                            parent,
                            layer,
                            node,
                        } => {
                            self.validate_scene_batch_alias(&alias)?;
                            if aliases.contains_key(&alias) {
                                return Err(ProgramSceneError::SceneBatchAliasDuplicate(alias));
                            }
                            let parent = parent
                                .as_ref()
                                .map(|parent| {
                                    self.resolve_scene_batch_node(session, parent, &aliases)
                                })
                                .transpose()?;
                            let ClpSceneCommandResult::Node { node, text_metrics } = self
                                .create_clp_node(session, parent, layer, node, scene, textures)?
                            else {
                                unreachable!("creating a CLP node returns a node result")
                            };
                            aliases.insert(alias.clone(), node);
                            nodes.insert(
                                alias.clone(),
                                SceneBatchCreatedNode { node, text_metrics },
                            );
                            SceneBatchOperationResult::Created { alias }
                        }
                        SceneBatchOperation::SetNode { node, changes } => {
                            let node = self.resolve_scene_batch_node(session, &node, &aliases)?;
                            let changes =
                                self.resolve_scene_batch_changes(session, changes, &aliases)?;
                            let ClpSceneCommandResult::NodeUpdated { text_metrics } =
                                self.set_clp_node(session, node, changes, scene, textures)?
                            else {
                                unreachable!("updating a CLP node returns an update result")
                            };
                            SceneBatchOperationResult::Updated { text_metrics }
                        }
                        SceneBatchOperation::RemoveNode { node } => {
                            let node = self.resolve_scene_batch_node(session, &node, &aliases)?;
                            self.execute(
                                session,
                                SceneCommand::RemoveNode { node },
                                scene,
                                textures,
                            )?;
                            SceneBatchOperationResult::Removed
                        }
                    })
                })()
                .map_err(|source| ProgramSceneError::SceneBatchOperation {
                    index,
                    alias,
                    source: Box::new(source),
                })?;
                results.push(result);
            }

            nodes.retain(|_, created| {
                self.nodes.get(&created.node).is_some_and(|owned| {
                    owned.session == session && scene.node(owned.node).is_some()
                })
            });

            Ok(SceneBatchResult {
                nodes,
                operations: results,
            })
        })();

        let result = match result {
            Ok(result) => result,
            Err(error) => {
                snapshot.restore(self, scene);
                return Err(error);
            }
        };

        self.overlap_pairs = snapshot.overlap_pairs.clone();
        self.pending_overlap_events = snapshot.pending_overlap_events.clone();
        if let Err(error) = self.reconcile_overlaps(session, scene) {
            snapshot.restore(self, scene);
            return Err(ProgramSceneError::SceneBatchOperation {
                index: operation_count,
                alias: None,
                source: Box::new(error),
            });
        }
        Ok(result)
    }

    fn validate_scene_batch_alias(&self, alias: &str) -> Result<(), ProgramSceneError> {
        if alias.is_empty() {
            return Err(ProgramSceneError::SceneBatchAliasInvalid(
                "it must not be empty",
            ));
        }
        if alias.len() > MAX_SCENE_BATCH_ALIAS_BYTES {
            return Err(ProgramSceneError::SceneBatchAliasInvalid(
                "it exceeds the byte limit",
            ));
        }
        if alias.chars().any(char::is_control) {
            return Err(ProgramSceneError::SceneBatchAliasInvalid(
                "it must not contain control characters",
            ));
        }
        Ok(())
    }

    fn resolve_scene_batch_node(
        &self,
        session: SessionId,
        reference: &SceneBatchNodeRef,
        aliases: &BTreeMap<String, NodeHandle>,
    ) -> Result<NodeHandle, ProgramSceneError> {
        match reference {
            SceneBatchNodeRef::Handle(handle) => {
                self.node_for(session, *handle)?;
                Ok(*handle)
            }
            SceneBatchNodeRef::Alias(alias) => {
                self.validate_scene_batch_alias(alias)?;
                let handle = aliases
                    .get(alias)
                    .copied()
                    .ok_or_else(|| ProgramSceneError::SceneBatchAliasUnknown(alias.clone()))?;
                if self
                    .nodes
                    .get(&handle)
                    .is_some_and(|owned| owned.session == session)
                {
                    Ok(handle)
                } else {
                    Err(ProgramSceneError::SceneBatchAliasRemoved(alias.clone()))
                }
            }
        }
    }

    fn resolve_scene_batch_changes(
        &self,
        session: SessionId,
        changes: SceneBatchNodeChanges,
        aliases: &BTreeMap<String, NodeHandle>,
    ) -> Result<NodeChanges, ProgramSceneError> {
        let parent = changes
            .parent
            .map(|parent| match parent {
                SceneBatchNodeParent::Root => Ok(NodeParent::Root),
                SceneBatchNodeParent::Node(node) => self
                    .resolve_scene_batch_node(session, &node, aliases)
                    .map(NodeParent::Node),
            })
            .transpose()?;
        Ok(NodeChanges {
            properties: changes.properties,
            parent,
            layer: changes.layer,
            visual: changes.visual,
            text: changes.text,
            text_style: changes.text_style,
        })
    }

    pub fn set_window_metadata(
        &mut self,
        session: SessionId,
        metadata: WindowMetadataPatch,
        textures: &TextureStore,
    ) -> Result<(), ProgramSceneError> {
        if let crate::window_metadata::MetadataField::Set(image) = &metadata.icon {
            let owned = self.image_for(session, *image)?;
            let ImageResource::Texture(texture) = owned.resource else {
                return Err(ProgramSceneError::WindowIconMustBeStaticRgba8(*image));
            };
            let texture = self.texture_for(session, texture)?;
            let texture = textures
                .texture(texture)
                .ok_or(ProgramSceneError::InvalidImageHandle(*image))?;
            if texture.width() > MAX_WINDOW_ICON_DIMENSION
                || texture.height() > MAX_WINDOW_ICON_DIMENSION
            {
                return Err(ProgramSceneError::WindowIconDimensionLimitExceeded {
                    limit: MAX_WINDOW_ICON_DIMENSION,
                });
            }
            if texture.pixels().len() > MAX_WINDOW_ICON_BYTES {
                return Err(ProgramSceneError::WindowIconByteLimitExceeded {
                    limit: MAX_WINDOW_ICON_BYTES,
                });
            }
        }
        self.window_metadata
            .apply(session, metadata)
            .map_err(ProgramSceneError::WindowMetadata)
    }

    pub fn active_window_metadata(&self) -> Option<&WindowMetadata> {
        self.window_metadata.active()
    }

    pub fn active_window_icon(&self, textures: &TextureStore) -> Option<WindowIcon> {
        let image = self.window_metadata.active()?.icon()?;
        let owned = self.images.get(&image)?;
        let ImageResource::Texture(texture) = owned.resource else {
            return None;
        };
        let texture = self.textures.get(&texture)?;
        let texture = textures.texture(texture.texture)?;
        Some(WindowIcon {
            width: texture.width(),
            height: texture.height(),
            pixels: texture.pixels().to_vec(),
        })
    }

    /// Applies a complete geometry packet only after every owned node and
    /// proposed geometry value has been validated. This preserves a single
    /// observable scene transition at the renderer frame boundary.
    pub fn present_clp(
        &mut self,
        session: SessionId,
        packet: &PresentationPacket,
        scene: &mut Scene,
    ) -> Result<(), ProgramSceneError> {
        if packet.updates.is_empty() {
            return Err(ProgramSceneError::InvalidPresentationUpdate(
                "a presentation packet must contain at least one node update",
            ));
        }
        if packet.updates.len() > MAX_PRESENTATION_UPDATES {
            return Err(ProgramSceneError::PresentationUpdateLimitExceeded {
                limit: MAX_PRESENTATION_UPDATES,
            });
        }

        let mut seen = BTreeSet::new();
        let mut property_changes = Vec::with_capacity(packet.updates.len());
        for update in &packet.updates {
            if !seen.insert(update.node) {
                return Err(ProgramSceneError::DuplicatePresentationNode(update.node));
            }
            let node = self.node_for(session, update.node)?;
            let current = scene
                .node(node)
                .ok_or(ProgramSceneError::InvalidNodeHandle(update.node))?;
            if matches!(current.content(), NodeContent::Text(_))
                && (update.properties.width.is_some() || update.properties.height.is_some())
            {
                return Err(ProgramSceneError::IntrinsicTextBounds(update.node));
            }
            property_changes.push((
                node,
                current.properties(),
                update.properties.apply(current.properties())?,
            ));
        }

        for (node, _, properties) in &property_changes {
            if let Err(error) = scene.set_properties(*node, *properties) {
                restore_presentation_properties(scene, &property_changes);
                return Err(ProgramSceneError::Scene(error));
            }
        }
        if let Err(error) = self.reconcile_overlaps(session, scene) {
            restore_presentation_properties(scene, &property_changes);
            return Err(error);
        }
        Ok(())
    }

    fn create_clp_node(
        &mut self,
        session: SessionId,
        parent: Option<NodeHandle>,
        layer: Option<SceneLayer>,
        node: NodeInput,
        scene: &mut Scene,
        textures: &mut TextureStore,
    ) -> Result<ClpSceneCommandResult, ProgramSceneError> {
        if parent.is_some() && layer.is_some() {
            return Err(ProgramSceneError::InvalidNodeChange(
                "only root nodes may specify a layer",
            ));
        }

        let result = match node {
            NodeInput::Empty { properties } => self.create_clp_visual_node(
                session,
                parent,
                layer.unwrap_or_default(),
                (properties, ContentInput::Empty),
                scene,
                textures,
            )?,
            NodeInput::Rectangle { properties, color } => self.create_clp_visual_node(
                session,
                parent,
                layer.unwrap_or_default(),
                (properties, ContentInput::Solid { color }),
                scene,
                textures,
            )?,
            NodeInput::Image {
                properties,
                image,
                opacity,
                contact_shape,
            } => self.create_clp_visual_node(
                session,
                parent,
                layer.unwrap_or_default(),
                (
                    properties,
                    self.resolve_clp_visual(
                        session,
                        NodeVisualInput::Image {
                            image,
                            opacity,
                            contact_shape,
                        },
                    )?,
                ),
                scene,
                textures,
            )?,
            NodeInput::Text {
                properties,
                text,
                style,
            } => match parent {
                Some(parent) => self.execute(
                    session,
                    SceneCommand::CreateTextChild {
                        parent,
                        properties,
                        text,
                        style,
                    },
                    scene,
                    textures,
                )?,
                None => self.execute(
                    session,
                    SceneCommand::CreateTextRoot {
                        layer: layer.unwrap_or_default(),
                        properties,
                        text,
                        style,
                    },
                    scene,
                    textures,
                )?,
            },
        };

        match result {
            SceneCommandResult::Node(node) => Ok(ClpSceneCommandResult::Node {
                node,
                text_metrics: None,
            }),
            SceneCommandResult::TextNode { node, metrics } => Ok(ClpSceneCommandResult::Node {
                node,
                text_metrics: Some(metrics),
            }),
            _ => unreachable!("creating a node returns a node handle"),
        }
    }

    fn create_clp_visual_node(
        &mut self,
        session: SessionId,
        parent: Option<NodeHandle>,
        layer: SceneLayer,
        node: (NodePropertiesInput, ContentInput),
        scene: &mut Scene,
        textures: &mut TextureStore,
    ) -> Result<SceneCommandResult, ProgramSceneError> {
        let (properties, content) = node;
        match parent {
            Some(parent) => self.execute(
                session,
                SceneCommand::CreateChild {
                    parent,
                    properties,
                    content,
                },
                scene,
                textures,
            ),
            None => self.execute(
                session,
                SceneCommand::CreateRoot {
                    layer,
                    properties,
                    content,
                },
                scene,
                textures,
            ),
        }
    }

    fn set_clp_node(
        &mut self,
        session: SessionId,
        handle: NodeHandle,
        changes: NodeChanges,
        scene: &mut Scene,
        textures: &TextureStore,
    ) -> Result<ClpSceneCommandResult, ProgramSceneError> {
        let node = self.node_for(session, handle)?;
        let (is_text, current_properties, current_parent) = {
            let current = scene
                .node(node)
                .ok_or(ProgramSceneError::InvalidNodeHandle(handle))?;
            (
                matches!(current.content(), NodeContent::Text(_)),
                current.properties(),
                current.parent(),
            )
        };
        if is_text && changes.visual.is_some() {
            return Err(ProgramSceneError::NodeKindMismatch(handle));
        }
        if !is_text && (changes.text.is_some() || changes.text_style.is_some()) {
            return Err(ProgramSceneError::NodeKindMismatch(handle));
        }
        if is_text
            && changes
                .properties
                .is_some_and(|properties| properties.width.is_some() || properties.height.is_some())
        {
            return Err(ProgramSceneError::IntrinsicTextBounds(handle));
        }

        let mut properties = changes
            .properties
            .map(|properties| properties.apply(current_properties))
            .transpose()?
            .unwrap_or(current_properties);
        self.validate_overlap_capacity(
            session,
            current_properties.overlap_enabled,
            properties.overlap_enabled,
            scene,
        )?;

        let parent = changes
            .parent
            .map(|parent| match parent {
                NodeParent::Root => Ok(None),
                NodeParent::Node(parent) => self.node_for(session, parent).map(Some),
            })
            .transpose()?
            .unwrap_or(current_parent);
        if changes.layer.is_some() && parent.is_some() {
            return Err(ProgramSceneError::InvalidNodeChange(
                "only root nodes may specify a layer",
            ));
        }
        scene
            .validate_reparent(node, parent)
            .map_err(ProgramSceneError::Scene)?;

        let visual = changes
            .visual
            .map(|visual| {
                self.resolve_clp_visual(session, visual)
                    .and_then(|visual| self.resolve_content(session, visual, textures))
            })
            .transpose()?;
        let text = if is_text && (changes.text.is_some() || changes.text_style.is_some()) {
            let existing = self.text_node_for(handle, node, scene)?;
            let (text, metrics) = self.create_text_node(
                session,
                changes.text.unwrap_or_else(|| existing.text().to_owned()),
                changes
                    .text_style
                    .unwrap_or_else(|| existing.style().clone()),
                scene,
                Some(&existing),
            )?;
            properties.bounds = Rect::new(0.0, 0.0, metrics.width, metrics.height)
                .map_err(ProgramSceneError::Geometry)?;
            Some((text, metrics))
        } else {
            None
        };

        scene
            .set_properties(node, properties)
            .expect("a validated scene node remains valid");
        if let Some(visual) = visual {
            scene
                .set_content(node, visual)
                .expect("a validated scene node remains valid");
        }
        if let Some((text, _)) = &text {
            scene
                .set_content(node, NodeContent::Text(text.clone()))
                .expect("a validated scene node remains valid");
        }
        if current_parent != parent {
            scene
                .reparent(node, parent)
                .expect("a validated scene hierarchy remains valid");
        }
        if let Some(layer) = changes.layer {
            scene
                .set_root_layer(node, layer)
                .expect("a validated root node remains valid");
        }
        self.reconcile_overlaps(session, scene)?;
        Ok(ClpSceneCommandResult::NodeUpdated {
            text_metrics: text.map(|(_, metrics)| metrics),
        })
    }

    pub fn execute(
        &mut self,
        session: SessionId,
        command: SceneCommand,
        scene: &mut Scene,
        textures: &mut TextureStore,
    ) -> Result<SceneCommandResult, ProgramSceneError> {
        match command {
            SceneCommand::CreateRoot {
                layer,
                properties,
                content,
            } => {
                self.validate_overlap_capacity(session, false, properties.overlap_enabled, scene)?;
                let content = self.resolve_content(session, content, textures)?;
                let node = scene.create_root_in_layer(properties.into_scene()?, layer);
                scene
                    .set_content(node, content)
                    .expect("a newly created scene root is valid");
                let handle = self.allocate_node(session, node)?;
                self.reconcile_overlaps(session, scene)?;
                Ok(SceneCommandResult::Node(handle))
            }
            SceneCommand::CreateChild {
                parent,
                properties,
                content,
            } => {
                self.validate_overlap_capacity(session, false, properties.overlap_enabled, scene)?;
                let parent = self.node_for(session, parent)?;
                let content = self.resolve_content(session, content, textures)?;
                let node = scene
                    .create_child(parent, properties.into_scene()?)
                    .map_err(ProgramSceneError::Scene)?;
                scene
                    .set_content(node, content)
                    .expect("a newly created scene child is valid");
                let handle = self.allocate_node(session, node)?;
                self.reconcile_overlaps(session, scene)?;
                Ok(SceneCommandResult::Node(handle))
            }
            SceneCommand::UpdateNode { node, properties } => {
                let handle = node;
                let node = self.node_for(session, handle)?;
                if matches!(
                    scene.node(node).map(|node| node.content()),
                    Some(NodeContent::Text(_))
                ) && (properties.width.is_some() || properties.height.is_some())
                {
                    return Err(ProgramSceneError::IntrinsicTextBounds(handle));
                }
                let current = scene
                    .node(node)
                    .ok_or(ProgramSceneError::InvalidNodeHandle(node_handle_for(
                        &self.nodes,
                        node,
                    )?))?
                    .properties();
                let next = properties.apply(current)?;
                self.validate_overlap_capacity(
                    session,
                    current.overlap_enabled,
                    next.overlap_enabled,
                    scene,
                )?;
                scene
                    .set_properties(node, next)
                    .map_err(ProgramSceneError::Scene)?;
                self.reconcile_overlaps(session, scene)?;
                Ok(SceneCommandResult::Acknowledged)
            }
            SceneCommand::SetContent { node, content } => {
                let handle = node;
                let node = self.node_for(session, handle)?;
                if matches!(
                    scene.node(node).map(|node| node.content()),
                    Some(NodeContent::Text(_))
                ) {
                    return Err(ProgramSceneError::TextContentRequiresTextCommands(handle));
                }
                self.set_content(session, node, content, scene, textures)?;
                self.reconcile_overlaps(session, scene)?;
                Ok(SceneCommandResult::Acknowledged)
            }
            SceneCommand::CreateTextRoot {
                layer,
                properties,
                text,
                style,
            } => {
                self.validate_overlap_capacity(session, false, properties.overlap_enabled, scene)?;
                let (text, metrics) = self.create_text_node(session, text, style, scene, None)?;
                let node = scene.create_root_in_layer(properties.into_scene(metrics)?, layer);
                scene
                    .set_content(node, NodeContent::Text(text))
                    .expect("a newly created text root is valid");
                let handle = self.allocate_node(session, node)?;
                self.reconcile_overlaps(session, scene)?;
                Ok(SceneCommandResult::TextNode {
                    node: handle,
                    metrics,
                })
            }
            SceneCommand::CreateTextChild {
                parent,
                properties,
                text,
                style,
            } => {
                self.validate_overlap_capacity(session, false, properties.overlap_enabled, scene)?;
                let parent = self.node_for(session, parent)?;
                let (text, metrics) = self.create_text_node(session, text, style, scene, None)?;
                let node = scene
                    .create_child(parent, properties.into_scene(metrics)?)
                    .map_err(ProgramSceneError::Scene)?;
                scene
                    .set_content(node, NodeContent::Text(text))
                    .expect("a newly created text child is valid");
                let handle = self.allocate_node(session, node)?;
                self.reconcile_overlaps(session, scene)?;
                Ok(SceneCommandResult::TextNode {
                    node: handle,
                    metrics,
                })
            }
            SceneCommand::SetText { node, text } => {
                let handle = node;
                let node = self.node_for(session, handle)?;
                let existing = self.text_node_for(handle, node, scene)?;
                let (text, metrics) = self.create_text_node(
                    session,
                    text,
                    existing.style().clone(),
                    scene,
                    Some(&existing),
                )?;
                self.replace_text_node(node, text, metrics, scene)?;
                self.reconcile_overlaps(session, scene)?;
                Ok(SceneCommandResult::TextMetrics(metrics))
            }
            SceneCommand::SetTextStyle { node, style } => {
                let handle = node;
                let node = self.node_for(session, handle)?;
                let existing = self.text_node_for(handle, node, scene)?;
                let (text, metrics) = self.create_text_node(
                    session,
                    existing.text().to_owned(),
                    style,
                    scene,
                    Some(&existing),
                )?;
                self.replace_text_node(node, text, metrics, scene)?;
                self.reconcile_overlaps(session, scene)?;
                Ok(SceneCommandResult::TextMetrics(metrics))
            }
            SceneCommand::Reparent { node, parent } => {
                let node = self.node_for(session, node)?;
                let parent = parent
                    .map(|handle| self.node_for(session, handle))
                    .transpose()?;
                scene
                    .reparent(node, parent)
                    .map_err(ProgramSceneError::Scene)?;
                self.reconcile_overlaps(session, scene)?;
                Ok(SceneCommandResult::Acknowledged)
            }
            SceneCommand::RemoveNode { node } => {
                let node = self.node_for(session, node)?;
                scene
                    .remove_subtree(node)
                    .map_err(ProgramSceneError::Scene)?;
                self.nodes
                    .retain(|_, owned| scene.node(owned.node).is_some());
                self.discard_missing_overlap_pairs();
                Ok(SceneCommandResult::Acknowledged)
            }
            SceneCommand::QueryOverlaps { node } => Ok(SceneCommandResult::Overlaps(
                self.overlaps_for(session, node, scene)?,
            )),
            SceneCommand::UploadTexture {
                width,
                height,
                pixels,
            } => {
                if width > self.max_texture_dimension || height > self.max_texture_dimension {
                    return Err(ProgramSceneError::TextureDimensionLimitExceeded {
                        limit: self.max_texture_dimension,
                    });
                }
                let bytes = rgba8_byte_len(width, height).map_err(ProgramSceneError::Texture)?;
                if bytes > MAX_TEXTURE_BYTES {
                    return Err(ProgramSceneError::TextureLimitExceeded {
                        limit: MAX_TEXTURE_BYTES,
                    });
                }
                let used = self
                    .session_texture_bytes
                    .get(&session)
                    .copied()
                    .unwrap_or_default();
                if used
                    .checked_add(bytes)
                    .is_none_or(|total| total > MAX_SESSION_TEXTURE_BYTES)
                {
                    return Err(ProgramSceneError::SessionTextureLimitExceeded {
                        limit: MAX_SESSION_TEXTURE_BYTES,
                    });
                }
                let texture = Texture::from_rgba8(width, height, pixels)
                    .map_err(ProgramSceneError::Texture)?;
                let texture = textures.insert(texture);
                let handle = self.allocate_texture(session, texture, bytes)?;
                self.session_texture_bytes.insert(session, used + bytes);
                Ok(SceneCommandResult::Texture(handle))
            }
            SceneCommand::DestroyTexture { texture } => {
                let texture_id = self.texture_for(session, texture)?;
                if scene.references_texture(texture_id) {
                    return Err(ProgramSceneError::TextureInUse(texture));
                }
                textures
                    .remove(texture_id)
                    .map_err(|_| ProgramSceneError::InvalidTextureHandle(texture))?;
                let owned = self
                    .textures
                    .remove(&texture)
                    .expect("a resolved texture handle is registered");
                self.reduce_session_bytes(session, owned.bytes);
                Ok(SceneCommandResult::Acknowledged)
            }
            SceneCommand::UploadAnimatedImage { format, bytes } => {
                let image = AnimatedImage::decode(format, bytes, self.max_texture_dimension)
                    .map_err(ProgramSceneError::AnimatedImage)?;
                let bytes = image.decoded_bytes();
                let used = self
                    .session_texture_bytes
                    .get(&session)
                    .copied()
                    .unwrap_or_default();
                if used
                    .checked_add(bytes)
                    .is_none_or(|total| total > MAX_SESSION_TEXTURE_BYTES)
                {
                    return Err(ProgramSceneError::SessionTextureLimitExceeded {
                        limit: MAX_SESSION_TEXTURE_BYTES,
                    });
                }
                let handle = self.allocate_animated_image()?;
                let image = self.animation_store.insert(image);
                self.animated_images.insert(
                    handle,
                    OwnedAnimatedImage {
                        session,
                        image,
                        bytes,
                    },
                );
                self.session_texture_bytes.insert(session, used + bytes);
                Ok(SceneCommandResult::AnimatedImage(handle))
            }
            SceneCommand::DestroyAnimatedImage { image } => {
                let image_id = self.animated_image_for(session, image)?;
                if scene.references_animated_image(image_id) {
                    return Err(ProgramSceneError::AnimatedImageInUse(image));
                }
                self.animation_store
                    .remove(image_id)
                    .ok_or(ProgramSceneError::InvalidAnimatedImageHandle(image))?;
                let owned = self
                    .animated_images
                    .remove(&image)
                    .expect("a resolved animated image handle is registered");
                self.reduce_session_bytes(session, owned.bytes);
                Ok(SceneCommandResult::Acknowledged)
            }
        }
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
    use std::{
        collections::HashSet,
        time::{Duration, Instant},
    };

    use image::{Delay, Frame, Rgba, RgbaImage, codecs::gif::GifEncoder};

    use super::*;
    use crate::protocol::SessionManager;

    fn session() -> SessionId {
        let mut manager = SessionManager::default();
        manager
            .accept(
                crate::protocol::ClientHello {
                    protocol_version: crate::protocol::CLP_PROTOCOL_VERSION,
                    requested_capabilities: crate::protocol::CapabilitySet::ALL,
                },
                Default::default(),
            )
            .unwrap()
            .0
    }

    fn session_pair() -> (SessionId, SessionId) {
        let mut manager = SessionManager::default();
        let hello = crate::protocol::ClientHello {
            protocol_version: crate::protocol::CLP_PROTOCOL_VERSION,
            requested_capabilities: crate::protocol::CapabilitySet::ALL,
        };
        let first = manager.accept(hello.clone(), Default::default()).unwrap().0;
        let second = manager.accept(hello, Default::default()).unwrap().0;
        (first, second)
    }

    fn properties() -> NodePropertiesInput {
        NodePropertiesInput {
            x: 1.0,
            y: 2.0,
            width: 30.0,
            height: 40.0,
            visible: true,
            clip_children: false,
            z_index: 0,
            overlap_enabled: false,
            external_drop_enabled: false,
            input_passthrough: false,
        }
    }

    fn text_properties(x: f64, y: f64) -> TextNodePropertiesInput {
        TextNodePropertiesInput {
            x,
            y,
            visible: true,
            clip_children: false,
            z_index: 0,
            overlap_enabled: false,
            external_drop_enabled: false,
            input_passthrough: false,
        }
    }

    fn animated_gif() -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = GifEncoder::new(&mut bytes);
        for color in [[255, 0, 0, 255], [0, 0, 255, 255]] {
            encoder
                .encode_frame(Frame::from_parts(
                    RgbaImage::from_pixel(1, 1, Rgba(color)),
                    0,
                    0,
                    Delay::from_numer_denom_ms(40, 1),
                ))
                .unwrap();
        }
        drop(encoder);
        bytes
    }

    fn transparent_then_opaque_gif() -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = GifEncoder::new(&mut bytes);
        for color in [[0, 0, 0, 0], [0, 0, 0, 255]] {
            encoder
                .encode_frame(Frame::from_parts(
                    RgbaImage::from_pixel(1, 1, Rgba(color)),
                    0,
                    0,
                    Delay::from_numer_denom_ms(40, 1),
                ))
                .unwrap();
        }
        drop(encoder);
        bytes
    }

    #[test]
    fn creates_text_nodes_with_intrinsic_terminal_cell_bounds() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();

        let (handle, metrics) = match resources
            .execute(
                session,
                SceneCommand::CreateTextRoot {
                    layer: SceneLayer::Overlay,
                    properties: text_properties(24.0, 48.0),
                    text: "a\tb\n界".to_owned(),
                    style: TextStyle::default(),
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::TextNode { node, metrics } => (node, metrics),
            _ => unreachable!(),
        };

        assert_eq!(metrics.columns, 9);
        assert_eq!(metrics.rows, 2);
        assert_eq!(metrics.cells, 11);
        assert!((metrics.width - 86.4).abs() < 0.001);
        assert_eq!(metrics.height, 40.0);
        let node = scene.roots()[0];
        assert_eq!(
            resources.node_handle_for_session(session, node),
            Some(handle)
        );
        let properties = scene.node(node).unwrap().properties();
        assert_eq!(properties.transform, Transform::new(24.0, 48.0).unwrap());
        assert_eq!(properties.bounds.width(), metrics.width);
        assert_eq!(properties.bounds.height(), metrics.height);
        assert!(matches!(
            scene.node(node).unwrap().content(),
            NodeContent::Text(text) if text.text() == "a\tb\n界"
        ));
    }

    #[test]
    fn updates_text_bounds_and_preserves_the_intrinsic_contract() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let handle = match resources
            .execute(
                session,
                SceneCommand::CreateTextRoot {
                    layer: SceneLayer::Overlay,
                    properties: text_properties(0.0, 0.0),
                    text: "x".to_owned(),
                    style: TextStyle::default(),
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::TextNode { node, .. } => node,
            _ => unreachable!(),
        };

        let metrics = match resources
            .execute(
                session,
                SceneCommand::SetText {
                    node: handle,
                    text: "x\n".to_owned(),
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::TextMetrics(metrics) => metrics,
            _ => unreachable!(),
        };
        assert_eq!(metrics.columns, 1);
        assert_eq!(metrics.rows, 2);
        assert!((metrics.width - 9.6).abs() < 0.001);
        assert_eq!(metrics.height, 40.0);
        assert_eq!(metrics.cells, 1);
        let node = scene.roots()[0];
        assert_eq!(scene.node(node).unwrap().properties().bounds.height(), 40.0);
        assert_eq!(
            resources.execute(
                session,
                SceneCommand::UpdateNode {
                    node: handle,
                    properties: NodePropertiesPatch {
                        width: Some(999.0),
                        ..NodePropertiesPatch::default()
                    },
                },
                &mut scene,
                &mut textures,
            ),
            Err(ProgramSceneError::IntrinsicTextBounds(handle))
        );
        assert_eq!(
            resources.execute(
                session,
                SceneCommand::SetContent {
                    node: handle,
                    content: ContentInput::Empty,
                },
                &mut scene,
                &mut textures,
            ),
            Err(ProgramSceneError::TextContentRequiresTextCommands(handle))
        );
    }

    #[test]
    fn text_changes_reconcile_opt_in_overlap_pairs() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let mut text_properties = text_properties(0.0, 0.0);
        text_properties.overlap_enabled = true;
        let text = match resources
            .execute(
                session,
                SceneCommand::CreateTextRoot {
                    layer: SceneLayer::Overlay,
                    properties: text_properties,
                    text: "x".to_owned(),
                    style: TextStyle::default(),
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::TextNode { node, .. } => node,
            _ => unreachable!(),
        };
        let target = match resources
            .execute(
                session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: NodePropertiesInput {
                        x: 80.0,
                        y: 0.0,
                        width: 10.0,
                        height: 10.0,
                        visible: true,
                        clip_children: false,
                        z_index: 0,
                        overlap_enabled: true,
                        external_drop_enabled: false,
                        input_passthrough: false,
                    },
                    content: ContentInput::Empty,
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::Node(node) => node,
            _ => unreachable!(),
        };
        assert!(resources.drain_overlap_events().is_empty());

        resources
            .execute(
                session,
                SceneCommand::SetText {
                    node: text,
                    text: "a\tb".to_owned(),
                },
                &mut scene,
                &mut textures,
            )
            .unwrap();
        assert!(matches!(
            resources.drain_overlap_events().as_slice(),
            [SceneEvent::OverlapEnter { .. }]
        ));
        assert_eq!(
            resources
                .execute(
                    session,
                    SceneCommand::QueryOverlaps { node: text },
                    &mut scene,
                    &mut textures,
                )
                .unwrap(),
            SceneCommandResult::Overlaps(vec![target])
        );
    }

    #[test]
    fn rejects_text_nodes_beyond_a_sessions_retained_node_limit() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        for _ in 0..MAX_SESSION_TEXT_NODES {
            assert!(matches!(
                resources.execute(
                    session,
                    SceneCommand::CreateTextRoot {
                        layer: SceneLayer::Overlay,
                        properties: text_properties(0.0, 0.0),
                        text: String::new(),
                        style: TextStyle::default(),
                    },
                    &mut scene,
                    &mut textures,
                ),
                Ok(SceneCommandResult::TextNode { .. })
            ));
        }

        assert_eq!(
            resources.execute(
                session,
                SceneCommand::CreateTextRoot {
                    layer: SceneLayer::Overlay,
                    properties: text_properties(0.0, 0.0),
                    text: String::new(),
                    style: TextStyle::default(),
                },
                &mut scene,
                &mut textures,
            ),
            Err(ProgramSceneError::SessionTextNodeLimitExceeded {
                limit: MAX_SESSION_TEXT_NODES,
            })
        );
        assert_eq!(scene.len(), MAX_SESSION_TEXT_NODES);
    }

    #[test]
    fn finds_the_topmost_owned_opt_in_external_drop_target() {
        let (first_session, second_session) = session_pair();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let properties_at = |z_index, external_drop_enabled| NodePropertiesInput {
            x: 0.0,
            y: 0.0,
            width: 20.0,
            height: 20.0,
            visible: true,
            clip_children: false,
            z_index,
            overlap_enabled: false,
            external_drop_enabled,
            input_passthrough: false,
        };

        resources
            .execute(
                first_session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: properties_at(0, true),
                    content: ContentInput::Empty,
                },
                &mut scene,
                &mut textures,
            )
            .unwrap();
        resources
            .execute(
                second_session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: properties_at(2, false),
                    content: ContentInput::Empty,
                },
                &mut scene,
                &mut textures,
            )
            .unwrap();
        let top = match resources
            .execute(
                second_session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: properties_at(1, true),
                    content: ContentInput::Empty,
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::Node(handle) => handle,
            _ => unreachable!(),
        };

        let point = crate::scene::Point::new(10.0, 10.0).unwrap();
        let target = resources
            .external_drop_target_at(&scene, point)
            .unwrap()
            .unwrap();
        assert_eq!(resources.session_for_node(target), Some(second_session));

        resources
            .execute(
                second_session,
                SceneCommand::UpdateNode {
                    node: top,
                    properties: NodePropertiesPatch {
                        visible: Some(false),
                        ..NodePropertiesPatch::default()
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap();
        let target = resources
            .external_drop_target_at(&scene, point)
            .unwrap()
            .unwrap();
        assert_eq!(resources.session_for_node(target), Some(first_session));
    }

    #[test]
    fn alpha_contact_controls_overlap_and_external_drop_targeting() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let upload = |resources: &mut ProgramSceneResources,
                      scene: &mut Scene,
                      textures: &mut TextureStore,
                      pixels| {
            match resources
                .execute(
                    session,
                    SceneCommand::UploadTexture {
                        width: 2,
                        height: 1,
                        pixels,
                    },
                    scene,
                    textures,
                )
                .unwrap()
            {
                SceneCommandResult::Texture(handle) => handle,
                _ => unreachable!(),
            }
        };
        let left_pixel = upload(
            &mut resources,
            &mut scene,
            &mut textures,
            vec![0, 0, 0, 255, 0, 0, 0, 0],
        );
        let right_pixel = upload(
            &mut resources,
            &mut scene,
            &mut textures,
            vec![0, 0, 0, 0, 0, 0, 0, 255],
        );
        let mut collider_properties = properties();
        collider_properties.x = 0.0;
        collider_properties.y = 0.0;
        collider_properties.width = 2.0;
        collider_properties.height = 1.0;
        collider_properties.overlap_enabled = true;
        let first = match resources
            .execute(
                session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: collider_properties,
                    content: ContentInput::Image {
                        texture: left_pixel,
                        opacity: 1.0,
                        contact_shape: ImageContactShape::alpha(),
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::Node(handle) => handle,
            _ => unreachable!(),
        };
        let second = match resources
            .execute(
                session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: collider_properties,
                    content: ContentInput::Image {
                        texture: right_pixel,
                        opacity: 1.0,
                        contact_shape: ImageContactShape::alpha(),
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::Node(handle) => handle,
            _ => unreachable!(),
        };
        assert!(resources.drain_overlap_events().is_empty());
        assert_eq!(
            resources
                .execute(
                    session,
                    SceneCommand::QueryOverlaps { node: first },
                    &mut scene,
                    &mut textures,
                )
                .unwrap(),
            SceneCommandResult::Overlaps(vec![])
        );

        resources
            .execute(
                session,
                SceneCommand::SetContent {
                    node: second,
                    content: ContentInput::Image {
                        texture: left_pixel,
                        opacity: 1.0,
                        contact_shape: ImageContactShape::alpha(),
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap();
        assert!(matches!(
            resources.drain_overlap_events().as_slice(),
            [SceneEvent::OverlapEnter { .. }]
        ));

        let mut back_properties = properties();
        back_properties.x = 0.0;
        back_properties.y = 10.0;
        back_properties.width = 2.0;
        back_properties.height = 1.0;
        back_properties.external_drop_enabled = true;
        let back = match resources
            .execute(
                session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: back_properties,
                    content: ContentInput::Empty,
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::Node(handle) => handle,
            _ => unreachable!(),
        };
        let mut front_properties = back_properties;
        front_properties.z_index = 1;
        let front = match resources
            .execute(
                session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: front_properties,
                    content: ContentInput::Image {
                        texture: left_pixel,
                        opacity: 1.0,
                        contact_shape: ImageContactShape::alpha(),
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::Node(handle) => handle,
            _ => unreachable!(),
        };
        let back_node = resources.node_for(session, back).unwrap();
        let front_node = resources.node_for(session, front).unwrap();
        assert_eq!(
            resources
                .external_drop_target_at(&scene, crate::scene::Point::new(1.5, 10.5).unwrap())
                .unwrap(),
            Some(back_node)
        );
        assert_eq!(
            resources
                .external_drop_target_at(&scene, crate::scene::Point::new(0.5, 10.5).unwrap())
                .unwrap(),
            Some(front_node)
        );

        resources
            .execute(
                session,
                SceneCommand::UpdateNode {
                    node: front,
                    properties: NodePropertiesPatch {
                        input_passthrough: Some(true),
                        ..NodePropertiesPatch::default()
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap();
        assert_eq!(
            resources
                .external_drop_target_at(&scene, crate::scene::Point::new(0.5, 10.5).unwrap())
                .unwrap(),
            Some(back_node)
        );
    }

    #[test]
    fn animated_alpha_contact_reconciles_when_the_current_gif_frame_changes() {
        let session = session();
        let mut resources = ProgramSceneResources::new(64);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let image = match resources
            .execute(
                session,
                SceneCommand::UploadAnimatedImage {
                    format: AnimatedImageFormat::Gif,
                    bytes: transparent_then_opaque_gif(),
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::AnimatedImage(handle) => handle,
            _ => unreachable!(),
        };
        let mut properties = properties();
        properties.x = 0.0;
        properties.y = 0.0;
        properties.width = 1.0;
        properties.height = 1.0;
        properties.overlap_enabled = true;
        let collider = match resources
            .execute(
                session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties,
                    content: ContentInput::Empty,
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::Node(handle) => handle,
            _ => unreachable!(),
        };
        let animated = match resources
            .execute(
                session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties,
                    content: ContentInput::AnimatedImage {
                        image,
                        opacity: 1.0,
                        contact_shape: ImageContactShape::alpha(),
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::Node(handle) => handle,
            _ => unreachable!(),
        };
        assert!(resources.drain_overlap_events().is_empty());

        let animated_node = resources.node_for(session, animated).unwrap();
        let image_id = match scene.node(animated_node).unwrap().content() {
            NodeContent::AnimatedImage(image) => image.image(),
            _ => unreachable!(),
        };
        let now = Instant::now();
        let visible = HashSet::from([image_id]);
        assert!(
            !resources
                .animated_images_mut()
                .update_playback(now, &visible)
        );
        assert!(
            resources
                .animated_images_mut()
                .update_playback(now + Duration::from_secs(1), &visible)
        );
        let changed = resources.sync_animated_alpha_contacts(&mut scene).unwrap();
        assert_eq!(changed, vec![animated_node]);
        resources
            .reconcile_animated_alpha_contacts(&changed, &scene)
            .unwrap();
        assert!(matches!(
            resources.drain_overlap_events().as_slice(),
            [SceneEvent::OverlapEnter { .. }]
        ));
        assert_eq!(
            resources
                .execute(
                    session,
                    SceneCommand::QueryOverlaps { node: animated },
                    &mut scene,
                    &mut textures,
                )
                .unwrap(),
            SceneCommandResult::Overlaps(vec![collider])
        );
    }

    #[test]
    fn owns_nodes_textures_and_cleans_up_a_closed_session() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let texture = match resources
            .execute(
                session,
                SceneCommand::UploadTexture {
                    width: 1,
                    height: 1,
                    pixels: vec![1, 2, 3, 4],
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::Texture(handle) => handle,
            _ => unreachable!(),
        };
        let root = match resources
            .execute(
                session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: properties(),
                    content: ContentInput::Image {
                        texture,
                        opacity: 1.0,
                        contact_shape: ImageContactShape::Bounds,
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::Node(handle) => handle,
            _ => unreachable!(),
        };
        assert_eq!(scene.len(), 1);
        assert_eq!(textures.len(), 1);
        let node = scene.roots()[0];
        assert_eq!(resources.node_handle_for_session(session, node), Some(root));
        assert_eq!(resources.session_for_node(node), Some(session));
        resources.cleanup_session(session, &mut scene, &mut textures);
        assert!(scene.is_empty());
        assert!(textures.is_empty());
        assert_eq!(resources.node_handle_for_session(session, node), None);
        assert_eq!(resources.session_for_node(node), None);
        assert!(matches!(
            resources.execute(
                session,
                SceneCommand::RemoveNode { node: root },
                &mut scene,
                &mut textures
            ),
            Err(ProgramSceneError::InvalidNodeHandle(_))
        ));
    }

    #[test]
    fn forbids_destroying_a_referenced_texture() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let texture = match resources
            .execute(
                session,
                SceneCommand::UploadTexture {
                    width: 1,
                    height: 1,
                    pixels: vec![0; 4],
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::Texture(handle) => handle,
            _ => unreachable!(),
        };
        resources
            .execute(
                session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: properties(),
                    content: ContentInput::Image {
                        texture,
                        opacity: 1.0,
                        contact_shape: ImageContactShape::Bounds,
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap();
        assert_eq!(
            resources.execute(
                session,
                SceneCommand::DestroyTexture { texture },
                &mut scene,
                &mut textures
            ),
            Err(ProgramSceneError::TextureInUse(texture))
        );
    }

    #[test]
    fn owns_animated_images_and_releases_them_after_node_removal() {
        let session = session();
        let mut resources = ProgramSceneResources::new(64);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let image = match resources
            .execute(
                session,
                SceneCommand::UploadAnimatedImage {
                    format: AnimatedImageFormat::Gif,
                    bytes: animated_gif(),
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::AnimatedImage(handle) => handle,
            _ => unreachable!(),
        };
        let node = match resources
            .execute(
                session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: properties(),
                    content: ContentInput::AnimatedImage {
                        image,
                        opacity: 1.0,
                        contact_shape: ImageContactShape::Bounds,
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::Node(handle) => handle,
            _ => unreachable!(),
        };
        assert_eq!(resources.animated_images().image_count(), 1);
        assert_eq!(
            resources.execute(
                session,
                SceneCommand::DestroyAnimatedImage { image },
                &mut scene,
                &mut textures,
            ),
            Err(ProgramSceneError::AnimatedImageInUse(image))
        );
        resources
            .execute(
                session,
                SceneCommand::RemoveNode { node },
                &mut scene,
                &mut textures,
            )
            .unwrap();
        resources
            .execute(
                session,
                SceneCommand::DestroyAnimatedImage { image },
                &mut scene,
                &mut textures,
            )
            .unwrap();
        assert_eq!(resources.animated_images().image_count(), 0);
    }

    #[test]
    fn session_cleanup_releases_animated_images_and_their_nodes() {
        let session = session();
        let mut resources = ProgramSceneResources::new(64);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let image = match resources
            .execute(
                session,
                SceneCommand::UploadAnimatedImage {
                    format: AnimatedImageFormat::Gif,
                    bytes: animated_gif(),
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::AnimatedImage(handle) => handle,
            _ => unreachable!(),
        };
        resources
            .execute(
                session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: properties(),
                    content: ContentInput::AnimatedImage {
                        image,
                        opacity: 1.0,
                        contact_shape: ImageContactShape::Bounds,
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap();

        resources.cleanup_session(session, &mut scene, &mut textures);

        assert_eq!(resources.animated_images().image_count(), 0);
        assert_eq!(scene.len(), 0);
    }

    #[test]
    fn rejected_animated_uploads_do_not_allocate_resources() {
        let session = session();
        let mut resources = ProgramSceneResources::new(64);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        resources
            .session_texture_bytes
            .insert(session, MAX_SESSION_TEXTURE_BYTES);

        assert_eq!(
            resources.execute(
                session,
                SceneCommand::UploadAnimatedImage {
                    format: AnimatedImageFormat::Gif,
                    bytes: animated_gif(),
                },
                &mut scene,
                &mut textures,
            ),
            Err(ProgramSceneError::SessionTextureLimitExceeded {
                limit: MAX_SESSION_TEXTURE_BYTES,
            })
        );
        assert_eq!(resources.animated_images().image_count(), 0);
        assert_eq!(scene.len(), 0);
    }

    #[test]
    fn exhausted_animated_handles_do_not_allocate_resources() {
        let session = session();
        let mut resources = ProgramSceneResources::new(64);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        resources.next_animated_image_handle = u64::MAX;

        assert_eq!(
            resources.execute(
                session,
                SceneCommand::UploadAnimatedImage {
                    format: AnimatedImageFormat::Gif,
                    bytes: animated_gif(),
                },
                &mut scene,
                &mut textures,
            ),
            Err(ProgramSceneError::HandleExhausted)
        );
        assert_eq!(resources.animated_images().image_count(), 0);
        assert_eq!(scene.len(), 0);
    }

    #[test]
    fn prevents_cross_session_animated_image_access() {
        let (owner, other) = session_pair();
        let mut resources = ProgramSceneResources::new(64);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let image = match resources
            .execute(
                owner,
                SceneCommand::UploadAnimatedImage {
                    format: AnimatedImageFormat::Gif,
                    bytes: animated_gif(),
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::AnimatedImage(handle) => handle,
            _ => unreachable!(),
        };

        assert_eq!(
            resources.execute(
                other,
                SceneCommand::DestroyAnimatedImage { image },
                &mut scene,
                &mut textures,
            ),
            Err(ProgramSceneError::HandleNotOwned)
        );
        assert_eq!(resources.animated_images().image_count(), 1);
    }

    #[test]
    fn rejects_handles_owned_by_another_session() {
        let (first, second) = session_pair();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let node = match resources
            .execute(
                first,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: properties(),
                    content: ContentInput::Solid {
                        color: [1, 2, 3, 4],
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::Node(handle) => handle,
            _ => unreachable!(),
        };
        assert_eq!(
            resources.execute(
                second,
                SceneCommand::RemoveNode { node },
                &mut scene,
                &mut textures
            ),
            Err(ProgramSceneError::HandleNotOwned)
        );
    }

    #[test]
    fn rejects_textures_beyond_the_active_gpu_dimension_limit() {
        let session = session();
        let mut resources = ProgramSceneResources::new(4);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();

        assert_eq!(
            resources.execute(
                session,
                SceneCommand::UploadTexture {
                    width: 5,
                    height: 1,
                    pixels: vec![0; 20],
                },
                &mut scene,
                &mut textures,
            ),
            Err(ProgramSceneError::TextureDimensionLimitExceeded { limit: 4 })
        );
        assert!(textures.is_empty());
    }

    #[test]
    fn does_not_create_a_node_when_its_content_is_invalid() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();

        assert!(matches!(
            resources.execute(
                session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: properties(),
                    content: ContentInput::Image {
                        texture: TextureHandle::new(1),
                        opacity: 1.0,
                        contact_shape: ImageContactShape::Bounds,
                    },
                },
                &mut scene,
                &mut textures,
            ),
            Err(ProgramSceneError::InvalidTextureHandle(_))
        ));
        assert!(scene.is_empty());
    }

    #[test]
    fn reports_opt_in_overlaps_without_pointer_input() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let collider = |x, y, visible| NodePropertiesInput {
            x,
            y,
            width: 10.0,
            height: 10.0,
            visible,
            clip_children: false,
            z_index: 0,
            overlap_enabled: true,
            external_drop_enabled: false,
            input_passthrough: false,
        };

        let first = match resources
            .execute(
                session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: collider(0.0, 0.0, true),
                    content: ContentInput::Empty,
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::Node(handle) => handle,
            _ => unreachable!(),
        };
        assert!(resources.drain_overlap_events().is_empty());

        let second = match resources
            .execute(
                session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: collider(5.0, 0.0, false),
                    content: ContentInput::Empty,
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::Node(handle) => handle,
            _ => unreachable!(),
        };
        assert!(matches!(
            resources.drain_overlap_events().as_slice(),
            [SceneEvent::OverlapEnter { .. }]
        ));
        assert_eq!(
            resources
                .execute(
                    session,
                    SceneCommand::QueryOverlaps { node: first },
                    &mut scene,
                    &mut textures,
                )
                .unwrap(),
            SceneCommandResult::Overlaps(vec![second])
        );

        resources
            .execute(
                session,
                SceneCommand::UpdateNode {
                    node: second,
                    properties: NodePropertiesPatch {
                        x: Some(10.0),
                        ..NodePropertiesPatch::default()
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap();
        assert!(matches!(
            resources.drain_overlap_events().as_slice(),
            [SceneEvent::OverlapLeave { .. }]
        ));
        assert_eq!(
            resources
                .execute(
                    session,
                    SceneCommand::QueryOverlaps { node: first },
                    &mut scene,
                    &mut textures,
                )
                .unwrap(),
            SceneCommandResult::Overlaps(vec![])
        );
    }

    #[test]
    fn overlap_queries_are_owned_and_the_source_need_not_be_a_collider() {
        let (first_session, second_session) = session_pair();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let properties_at = |x, overlap_enabled| NodePropertiesInput {
            x,
            y: 0.0,
            width: 10.0,
            height: 10.0,
            visible: true,
            clip_children: false,
            z_index: 0,
            overlap_enabled,
            external_drop_enabled: false,
            input_passthrough: false,
        };

        let source = match resources
            .execute(
                first_session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: properties_at(0.0, false),
                    content: ContentInput::Empty,
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::Node(handle) => handle,
            _ => unreachable!(),
        };
        let target = match resources
            .execute(
                first_session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: properties_at(5.0, true),
                    content: ContentInput::Empty,
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::Node(handle) => handle,
            _ => unreachable!(),
        };
        let _foreign = resources
            .execute(
                second_session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: properties_at(5.0, true),
                    content: ContentInput::Empty,
                },
                &mut scene,
                &mut textures,
            )
            .unwrap();

        assert!(resources.drain_overlap_events().is_empty());
        assert_eq!(
            resources
                .execute(
                    first_session,
                    SceneCommand::QueryOverlaps { node: source },
                    &mut scene,
                    &mut textures,
                )
                .unwrap(),
            SceneCommandResult::Overlaps(vec![target])
        );
    }

    #[test]
    fn overlap_reconciliation_includes_descendants_moved_by_an_ancestor() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let properties_at = |x, overlap_enabled| NodePropertiesInput {
            x,
            y: 0.0,
            width: 10.0,
            height: 10.0,
            visible: true,
            clip_children: false,
            z_index: 0,
            overlap_enabled,
            external_drop_enabled: false,
            input_passthrough: false,
        };
        let _stationary = resources
            .execute(
                session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: properties_at(0.0, true),
                    content: ContentInput::Empty,
                },
                &mut scene,
                &mut textures,
            )
            .unwrap();
        let parent = match resources
            .execute(
                session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: properties_at(20.0, false),
                    content: ContentInput::Empty,
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::Node(handle) => handle,
            _ => unreachable!(),
        };
        let _child = resources
            .execute(
                session,
                SceneCommand::CreateChild {
                    parent,
                    properties: properties_at(0.0, true),
                    content: ContentInput::Empty,
                },
                &mut scene,
                &mut textures,
            )
            .unwrap();
        assert!(resources.drain_overlap_events().is_empty());

        resources
            .execute(
                session,
                SceneCommand::UpdateNode {
                    node: parent,
                    properties: NodePropertiesPatch {
                        x: Some(5.0),
                        ..NodePropertiesPatch::default()
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap();
        assert!(matches!(
            resources.drain_overlap_events().as_slice(),
            [SceneEvent::OverlapEnter { .. }]
        ));
    }

    #[test]
    fn removing_a_node_cancels_overlap_pairs_without_a_leave_event() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let collider = |x| NodePropertiesInput {
            x,
            y: 0.0,
            width: 10.0,
            height: 10.0,
            visible: true,
            clip_children: false,
            z_index: 0,
            overlap_enabled: true,
            external_drop_enabled: false,
            input_passthrough: false,
        };
        let first = match resources
            .execute(
                session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: collider(0.0),
                    content: ContentInput::Empty,
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::Node(handle) => handle,
            _ => unreachable!(),
        };
        let second = match resources
            .execute(
                session,
                SceneCommand::CreateRoot {
                    layer: SceneLayer::Overlay,
                    properties: collider(5.0),
                    content: ContentInput::Empty,
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            SceneCommandResult::Node(handle) => handle,
            _ => unreachable!(),
        };
        assert!(matches!(
            resources.drain_overlap_events().as_slice(),
            [SceneEvent::OverlapEnter { .. }]
        ));

        resources
            .execute(
                session,
                SceneCommand::RemoveNode { node: second },
                &mut scene,
                &mut textures,
            )
            .unwrap();
        assert!(resources.drain_overlap_events().is_empty());
        assert_eq!(
            resources
                .execute(
                    session,
                    SceneCommand::QueryOverlaps { node: first },
                    &mut scene,
                    &mut textures,
                )
                .unwrap(),
            SceneCommandResult::Overlaps(vec![])
        );
    }

    #[test]
    fn clp_v2_unifies_images_and_rejects_partial_node_changes() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let image = match resources
            .execute_clp(
                session,
                ClpSceneCommand::UploadImage {
                    image: ImageUpload::Rgba8 {
                        width: 1,
                        height: 1,
                        pixels: vec![255, 127, 0, 255],
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            ClpSceneCommandResult::Image(image) => image,
            _ => unreachable!(),
        };
        let node = match resources
            .execute_clp(
                session,
                ClpSceneCommand::CreateNode {
                    parent: None,
                    layer: Some(SceneLayer::Overlay),
                    node: NodeInput::Rectangle {
                        properties: properties(),
                        color: [0, 0, 255, 255],
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            ClpSceneCommandResult::Node { node, .. } => node,
            _ => unreachable!(),
        };
        assert_eq!(
            resources.execute_clp(
                session,
                ClpSceneCommand::SetNode {
                    node,
                    changes: NodeChanges {
                        properties: Some(NodePropertiesPatch {
                            x: Some(99.0),
                            ..NodePropertiesPatch::default()
                        }),
                        text: Some("not allowed".to_owned()),
                        ..NodeChanges::default()
                    },
                },
                &mut scene,
                &mut textures,
            ),
            Err(ProgramSceneError::NodeKindMismatch(node))
        );
        let scene_node = resources.node_for(session, node).unwrap();
        assert_eq!(
            scene.node(scene_node).unwrap().properties().transform.x(),
            1.0
        );

        resources
            .execute_clp(
                session,
                ClpSceneCommand::SetNode {
                    node,
                    changes: NodeChanges {
                        visual: Some(NodeVisualInput::Image {
                            image,
                            opacity: 0.5,
                            contact_shape: ImageContactShape::Bounds,
                        }),
                        ..NodeChanges::default()
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap();
        assert!(matches!(
            resources.execute_clp(
                session,
                ClpSceneCommand::RemoveImage { image },
                &mut scene,
                &mut textures,
            ),
            Err(ProgramSceneError::ImageInUse(handle)) if handle == image
        ));
        resources
            .execute_clp(
                session,
                ClpSceneCommand::SetNode {
                    node,
                    changes: NodeChanges {
                        visual: Some(NodeVisualInput::Rectangle {
                            color: [0, 0, 255, 255],
                        }),
                        ..NodeChanges::default()
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap();
        assert_eq!(
            resources
                .execute_clp(
                    session,
                    ClpSceneCommand::RemoveImage { image },
                    &mut scene,
                    &mut textures,
                )
                .unwrap(),
            ClpSceneCommandResult::Acknowledged
        );
    }

    #[test]
    fn presentation_packets_validate_atomically_before_changing_geometry() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let node = match resources
            .execute_clp(
                session,
                ClpSceneCommand::CreateNode {
                    parent: None,
                    layer: Some(SceneLayer::Overlay),
                    node: NodeInput::Rectangle {
                        properties: properties(),
                        color: [255, 127, 0, 255],
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            ClpSceneCommandResult::Node { node, .. } => node,
            _ => unreachable!(),
        };
        let second_node = match resources
            .execute_clp(
                session,
                ClpSceneCommand::CreateNode {
                    parent: None,
                    layer: Some(SceneLayer::Overlay),
                    node: NodeInput::Rectangle {
                        properties: properties(),
                        color: [0, 127, 255, 255],
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            ClpSceneCommandResult::Node { node, .. } => node,
            _ => unreachable!(),
        };

        let duplicate = PresentationPacket {
            presentation_id: "duplicate".to_owned(),
            updates: vec![
                PresentationUpdate {
                    node,
                    properties: PresentationPropertiesPatch {
                        x: Some(99.0),
                        ..PresentationPropertiesPatch::default()
                    },
                },
                PresentationUpdate {
                    node,
                    properties: PresentationPropertiesPatch {
                        y: Some(77.0),
                        ..PresentationPropertiesPatch::default()
                    },
                },
            ],
        };
        assert_eq!(
            resources.present_clp(session, &duplicate, &mut scene),
            Err(ProgramSceneError::DuplicatePresentationNode(node))
        );
        let scene_node = resources.node_for(session, node).unwrap();
        assert_eq!(
            scene.node(scene_node).unwrap().properties().transform.x(),
            1.0
        );
        assert_eq!(
            scene.node(scene_node).unwrap().properties().transform.y(),
            2.0
        );

        let invalid_geometry = PresentationPacket {
            presentation_id: "invalid-geometry".to_owned(),
            updates: vec![
                PresentationUpdate {
                    node,
                    properties: PresentationPropertiesPatch {
                        x: Some(55.0),
                        ..PresentationPropertiesPatch::default()
                    },
                },
                PresentationUpdate {
                    node: second_node,
                    properties: PresentationPropertiesPatch {
                        y: Some(f64::NAN),
                        ..PresentationPropertiesPatch::default()
                    },
                },
            ],
        };
        assert!(matches!(
            resources.present_clp(session, &invalid_geometry, &mut scene),
            Err(ProgramSceneError::Geometry(_))
        ));
        let second_scene_node = resources.node_for(session, second_node).unwrap();
        assert_eq!(
            scene.node(scene_node).unwrap().properties().transform.x(),
            1.0
        );
        assert_eq!(
            scene
                .node(second_scene_node)
                .unwrap()
                .properties()
                .transform
                .y(),
            2.0
        );

        let valid = PresentationPacket {
            presentation_id: "valid".to_owned(),
            updates: vec![PresentationUpdate {
                node,
                properties: PresentationPropertiesPatch {
                    x: Some(99.0),
                    y: Some(77.0),
                    ..PresentationPropertiesPatch::default()
                },
            }],
        };
        resources.present_clp(session, &valid, &mut scene).unwrap();
        assert_eq!(
            scene.node(scene_node).unwrap().properties().transform.x(),
            99.0
        );
        assert_eq!(
            scene.node(scene_node).unwrap().properties().transform.y(),
            77.0
        );
    }

    #[test]
    fn presentation_packets_preserve_text_intrinsic_bounds_and_session_ownership() {
        let (owner, other) = session_pair();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let text = match resources
            .execute_clp(
                owner,
                ClpSceneCommand::CreateNode {
                    parent: None,
                    layer: Some(SceneLayer::Overlay),
                    node: NodeInput::Text {
                        properties: text_properties(4.0, 5.0),
                        text: "text".to_owned(),
                        style: TextStyle::default(),
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            ClpSceneCommandResult::Node { node, .. } => node,
            _ => unreachable!(),
        };
        let text_node = resources.node_for(owner, text).unwrap();

        let bounds = PresentationPacket {
            presentation_id: "text-bounds".to_owned(),
            updates: vec![PresentationUpdate {
                node: text,
                properties: PresentationPropertiesPatch {
                    width: Some(99.0),
                    ..PresentationPropertiesPatch::default()
                },
            }],
        };
        assert_eq!(
            resources.present_clp(owner, &bounds, &mut scene),
            Err(ProgramSceneError::IntrinsicTextBounds(text))
        );
        assert_eq!(
            scene.node(text_node).unwrap().properties().transform.x(),
            4.0
        );

        let foreign = PresentationPacket {
            presentation_id: "foreign".to_owned(),
            updates: vec![PresentationUpdate {
                node: text,
                properties: PresentationPropertiesPatch {
                    x: Some(60.0),
                    ..PresentationPropertiesPatch::default()
                },
            }],
        };
        assert_eq!(
            resources.present_clp(other, &foreign, &mut scene),
            Err(ProgramSceneError::HandleNotOwned)
        );
        assert_eq!(
            scene.node(text_node).unwrap().properties().transform.x(),
            4.0
        );
    }

    #[test]
    fn clp_v2_reports_boolean_and_complete_overlap_queries() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let collider = |x| NodeInput::Empty {
            properties: NodePropertiesInput {
                x,
                y: 0.0,
                width: 10.0,
                height: 10.0,
                visible: true,
                clip_children: false,
                z_index: 0,
                overlap_enabled: true,
                external_drop_enabled: false,
                input_passthrough: false,
            },
        };
        let create = |resources: &mut ProgramSceneResources,
                      scene: &mut Scene,
                      textures: &mut TextureStore,
                      node| {
            match resources
                .execute_clp(
                    session,
                    ClpSceneCommand::CreateNode {
                        parent: None,
                        layer: None,
                        node,
                    },
                    scene,
                    textures,
                )
                .unwrap()
            {
                ClpSceneCommandResult::Node { node, .. } => node,
                _ => unreachable!(),
            }
        };
        let first = create(&mut resources, &mut scene, &mut textures, collider(0.0));
        let second = create(&mut resources, &mut scene, &mut textures, collider(5.0));

        assert_eq!(
            resources
                .execute_clp(
                    session,
                    ClpSceneCommand::IsOverlapping { node: first },
                    &mut scene,
                    &mut textures,
                )
                .unwrap(),
            ClpSceneCommandResult::IsOverlapping(true)
        );
        assert_eq!(
            resources
                .execute_clp(
                    session,
                    ClpSceneCommand::GetOverlaps { node: first },
                    &mut scene,
                    &mut textures,
                )
                .unwrap(),
            ClpSceneCommandResult::Overlaps(vec![second])
        );
    }

    #[test]
    fn clp_scene_batches_build_trees_and_return_alias_handles() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();

        let result = resources
            .execute_clp(
                session,
                ClpSceneCommand::SceneBatch {
                    operations: vec![
                        SceneBatchOperation::CreateNode {
                            alias: "panel".to_owned(),
                            parent: None,
                            layer: Some(SceneLayer::Overlay),
                            node: NodeInput::Rectangle {
                                properties: NodePropertiesInput {
                                    x: 8.0,
                                    y: 12.0,
                                    width: 240.0,
                                    height: 120.0,
                                    visible: true,
                                    clip_children: false,
                                    z_index: 3,
                                    overlap_enabled: false,
                                    external_drop_enabled: false,
                                    input_passthrough: false,
                                },
                                color: [36, 88, 216, 255],
                            },
                        },
                        SceneBatchOperation::CreateNode {
                            alias: "label".to_owned(),
                            parent: Some(SceneBatchNodeRef::Alias("panel".to_owned())),
                            layer: None,
                            node: NodeInput::Text {
                                properties: text_properties(16.0, 24.0),
                                text: "Ready".to_owned(),
                                style: TextStyle::default(),
                            },
                        },
                        SceneBatchOperation::SetNode {
                            node: SceneBatchNodeRef::Alias("panel".to_owned()),
                            changes: SceneBatchNodeChanges {
                                properties: Some(NodePropertiesPatch {
                                    x: Some(20.0),
                                    clip_children: Some(true),
                                    ..NodePropertiesPatch::default()
                                }),
                                ..SceneBatchNodeChanges::default()
                            },
                        },
                    ],
                },
                &mut scene,
                &mut textures,
            )
            .unwrap();
        let ClpSceneCommandResult::SceneBatch(batch) = result else {
            unreachable!("a scene batch returns its created node handles")
        };

        assert_eq!(
            batch.operations,
            vec![
                SceneBatchOperationResult::Created {
                    alias: "panel".to_owned(),
                },
                SceneBatchOperationResult::Created {
                    alias: "label".to_owned(),
                },
                SceneBatchOperationResult::Updated { text_metrics: None },
            ]
        );
        let panel = batch.nodes.get("panel").unwrap().node;
        let label = batch.nodes.get("label").unwrap().node;
        let metrics = batch.nodes.get("label").unwrap().text_metrics.unwrap();
        assert_eq!(panel, NodeHandle::new(1));
        assert_eq!(label, NodeHandle::new(2));
        assert_eq!(metrics.columns, 5);
        assert_eq!(metrics.rows, 1);

        let panel_node = resources.node_for(session, panel).unwrap();
        let label_node = resources.node_for(session, label).unwrap();
        assert_eq!(scene.len(), 2);
        assert_eq!(scene.node(panel_node).unwrap().layer(), SceneLayer::Overlay);
        assert_eq!(scene.node(label_node).unwrap().parent(), Some(panel_node));
        assert_eq!(
            scene.node(panel_node).unwrap().properties().transform.x(),
            20.0
        );
        assert!(scene.node(panel_node).unwrap().properties().clip_children);
    }

    #[test]
    fn clp_scene_batches_allow_temporary_nodes_to_be_removed() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();

        let result = resources
            .execute_clp(
                session,
                ClpSceneCommand::SceneBatch {
                    operations: vec![
                        SceneBatchOperation::CreateNode {
                            alias: "temporary".to_owned(),
                            parent: None,
                            layer: Some(SceneLayer::Overlay),
                            node: NodeInput::Empty {
                                properties: properties(),
                            },
                        },
                        SceneBatchOperation::RemoveNode {
                            node: SceneBatchNodeRef::Alias("temporary".to_owned()),
                        },
                    ],
                },
                &mut scene,
                &mut textures,
            )
            .unwrap();
        let ClpSceneCommandResult::SceneBatch(batch) = result else {
            unreachable!("a scene batch returns its operation results")
        };

        assert!(batch.nodes.is_empty());
        assert_eq!(
            batch.operations,
            vec![
                SceneBatchOperationResult::Created {
                    alias: "temporary".to_owned(),
                },
                SceneBatchOperationResult::Removed,
            ]
        );
        assert!(scene.is_empty());

        let error = resources
            .execute_clp(
                session,
                ClpSceneCommand::SceneBatch {
                    operations: vec![
                        SceneBatchOperation::CreateNode {
                            alias: "temporary".to_owned(),
                            parent: None,
                            layer: Some(SceneLayer::Overlay),
                            node: NodeInput::Empty {
                                properties: properties(),
                            },
                        },
                        SceneBatchOperation::RemoveNode {
                            node: SceneBatchNodeRef::Alias("temporary".to_owned()),
                        },
                        SceneBatchOperation::SetNode {
                            node: SceneBatchNodeRef::Alias("temporary".to_owned()),
                            changes: SceneBatchNodeChanges::default(),
                        },
                    ],
                },
                &mut scene,
                &mut textures,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ProgramSceneError::SceneBatchOperation {
                index: 2,
                alias: None,
                source,
            } if matches!(*source, ProgramSceneError::SceneBatchAliasRemoved(ref alias) if alias == "temporary")
        ));
        assert!(scene.is_empty());
    }

    #[test]
    fn clp_scene_batches_roll_back_all_state_and_handle_allocation_on_failure() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let existing = match resources
            .execute_clp(
                session,
                ClpSceneCommand::CreateNode {
                    parent: None,
                    layer: Some(SceneLayer::Overlay),
                    node: NodeInput::Rectangle {
                        properties: properties(),
                        color: [36, 88, 216, 255],
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            ClpSceneCommandResult::Node { node, .. } => node,
            _ => unreachable!(),
        };
        let before = scene.clone();

        let error = resources
            .execute_clp(
                session,
                ClpSceneCommand::SceneBatch {
                    operations: vec![
                        SceneBatchOperation::CreateNode {
                            alias: "temporary".to_owned(),
                            parent: None,
                            layer: Some(SceneLayer::Overlay),
                            node: NodeInput::Empty {
                                properties: properties(),
                            },
                        },
                        SceneBatchOperation::SetNode {
                            node: SceneBatchNodeRef::Handle(existing),
                            changes: SceneBatchNodeChanges {
                                properties: Some(NodePropertiesPatch {
                                    x: Some(f64::NAN),
                                    ..NodePropertiesPatch::default()
                                }),
                                ..SceneBatchNodeChanges::default()
                            },
                        },
                    ],
                },
                &mut scene,
                &mut textures,
            )
            .unwrap_err();
        match error {
            ProgramSceneError::SceneBatchOperation {
                index,
                alias,
                source,
            } => {
                assert_eq!(index, 1);
                assert_eq!(alias, None);
                assert!(matches!(*source, ProgramSceneError::Geometry(_)));
            }
            error => panic!("expected a batch operation error, got {error:?}"),
        }
        assert_eq!(scene, before);

        let next = match resources
            .execute_clp(
                session,
                ClpSceneCommand::CreateNode {
                    parent: None,
                    layer: Some(SceneLayer::Overlay),
                    node: NodeInput::Empty {
                        properties: properties(),
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            ClpSceneCommandResult::Node { node, .. } => node,
            _ => unreachable!(),
        };
        assert_eq!(next, NodeHandle::new(2));
    }

    #[test]
    fn clp_scene_batches_do_not_retain_images_after_a_rejected_batch() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let image = match resources
            .execute_clp(
                session,
                ClpSceneCommand::UploadImage {
                    image: ImageUpload::Rgba8 {
                        width: 1,
                        height: 1,
                        pixels: vec![255, 127, 0, 255],
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            ClpSceneCommandResult::Image(image) => image,
            _ => unreachable!(),
        };

        let error = resources
            .execute_clp(
                session,
                ClpSceneCommand::SceneBatch {
                    operations: vec![
                        SceneBatchOperation::CreateNode {
                            alias: "image".to_owned(),
                            parent: None,
                            layer: Some(SceneLayer::Overlay),
                            node: NodeInput::Image {
                                properties: properties(),
                                image,
                                opacity: 1.0,
                                contact_shape: ImageContactShape::Bounds,
                            },
                        },
                        SceneBatchOperation::SetNode {
                            node: SceneBatchNodeRef::Alias("image".to_owned()),
                            changes: SceneBatchNodeChanges {
                                properties: Some(NodePropertiesPatch {
                                    x: Some(f64::NAN),
                                    ..NodePropertiesPatch::default()
                                }),
                                ..SceneBatchNodeChanges::default()
                            },
                        },
                    ],
                },
                &mut scene,
                &mut textures,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ProgramSceneError::SceneBatchOperation {
                index: 1,
                alias: None,
                source,
            } if matches!(*source, ProgramSceneError::Geometry(_))
        ));
        assert!(scene.is_empty());

        assert_eq!(
            resources
                .execute_clp(
                    session,
                    ClpSceneCommand::RemoveImage { image },
                    &mut scene,
                    &mut textures,
                )
                .unwrap(),
            ClpSceneCommandResult::Acknowledged
        );
    }

    #[test]
    fn clp_scene_batches_preserve_session_ownership_when_rolling_back() {
        let (session, foreign_session) = session_pair();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let foreign = match resources
            .execute_clp(
                foreign_session,
                ClpSceneCommand::CreateNode {
                    parent: None,
                    layer: Some(SceneLayer::Overlay),
                    node: NodeInput::Empty {
                        properties: properties(),
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            ClpSceneCommandResult::Node { node, .. } => node,
            _ => unreachable!(),
        };
        let before = scene.clone();

        let error = resources
            .execute_clp(
                session,
                ClpSceneCommand::SceneBatch {
                    operations: vec![
                        SceneBatchOperation::CreateNode {
                            alias: "local".to_owned(),
                            parent: None,
                            layer: Some(SceneLayer::Overlay),
                            node: NodeInput::Empty {
                                properties: properties(),
                            },
                        },
                        SceneBatchOperation::SetNode {
                            node: SceneBatchNodeRef::Handle(foreign),
                            changes: SceneBatchNodeChanges::default(),
                        },
                    ],
                },
                &mut scene,
                &mut textures,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ProgramSceneError::SceneBatchOperation {
                index: 1,
                alias: None,
                source,
            } if matches!(*source, ProgramSceneError::HandleNotOwned)
        ));
        assert_eq!(scene, before);

        let next = match resources
            .execute_clp(
                session,
                ClpSceneCommand::CreateNode {
                    parent: None,
                    layer: Some(SceneLayer::Overlay),
                    node: NodeInput::Empty {
                        properties: properties(),
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            ClpSceneCommandResult::Node { node, .. } => node,
            _ => unreachable!(),
        };
        assert_eq!(next, NodeHandle::new(2));
    }

    #[test]
    fn clp_scene_batches_require_prior_unique_aliases_and_respect_the_operation_limit() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();

        let forward_reference = resources
            .execute_clp(
                session,
                ClpSceneCommand::SceneBatch {
                    operations: vec![SceneBatchOperation::CreateNode {
                        alias: "child".to_owned(),
                        parent: Some(SceneBatchNodeRef::Alias("parent".to_owned())),
                        layer: None,
                        node: NodeInput::Empty {
                            properties: properties(),
                        },
                    }],
                },
                &mut scene,
                &mut textures,
            )
            .unwrap_err();
        assert!(matches!(
            forward_reference,
            ProgramSceneError::SceneBatchOperation {
                index: 0,
                alias: Some(alias),
                source,
            } if alias == "child"
                && matches!(*source, ProgramSceneError::SceneBatchAliasUnknown(ref parent) if parent == "parent")
        ));
        assert!(scene.is_empty());

        let duplicate_alias = resources
            .execute_clp(
                session,
                ClpSceneCommand::SceneBatch {
                    operations: vec![
                        SceneBatchOperation::CreateNode {
                            alias: "same".to_owned(),
                            parent: None,
                            layer: Some(SceneLayer::Overlay),
                            node: NodeInput::Empty {
                                properties: properties(),
                            },
                        },
                        SceneBatchOperation::CreateNode {
                            alias: "same".to_owned(),
                            parent: None,
                            layer: Some(SceneLayer::Overlay),
                            node: NodeInput::Empty {
                                properties: properties(),
                            },
                        },
                    ],
                },
                &mut scene,
                &mut textures,
            )
            .unwrap_err();
        assert!(matches!(
            duplicate_alias,
            ProgramSceneError::SceneBatchOperation {
                index: 1,
                alias: Some(alias),
                source,
            } if alias == "same"
                && matches!(*source, ProgramSceneError::SceneBatchAliasDuplicate(ref duplicate) if duplicate == "same")
        ));
        assert!(scene.is_empty());

        let too_many_operations = (0..=MAX_SCENE_BATCH_OPERATIONS)
            .map(|index| SceneBatchOperation::CreateNode {
                alias: format!("node-{index}"),
                parent: None,
                layer: Some(SceneLayer::Overlay),
                node: NodeInput::Empty {
                    properties: properties(),
                },
            })
            .collect();
        assert_eq!(
            resources.execute_clp(
                session,
                ClpSceneCommand::SceneBatch {
                    operations: too_many_operations,
                },
                &mut scene,
                &mut textures,
            ),
            Err(ProgramSceneError::SceneBatchOperationLimitExceeded {
                limit: MAX_SCENE_BATCH_OPERATIONS,
            })
        );
        assert!(scene.is_empty());
    }

    #[test]
    fn clp_scene_batches_emit_overlap_events_only_for_the_final_scene() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let collider = |x| NodeInput::Empty {
            properties: NodePropertiesInput {
                x,
                y: 0.0,
                width: 10.0,
                height: 10.0,
                visible: true,
                clip_children: false,
                z_index: 0,
                overlap_enabled: true,
                external_drop_enabled: false,
                input_passthrough: false,
            },
        };
        let anchor = match resources
            .execute_clp(
                session,
                ClpSceneCommand::CreateNode {
                    parent: None,
                    layer: Some(SceneLayer::Overlay),
                    node: collider(0.0),
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            ClpSceneCommandResult::Node { node, .. } => node,
            _ => unreachable!(),
        };
        assert!(resources.drain_overlap_events().is_empty());

        let batch = match resources
            .execute_clp(
                session,
                ClpSceneCommand::SceneBatch {
                    operations: vec![
                        SceneBatchOperation::CreateNode {
                            alias: "transient".to_owned(),
                            parent: None,
                            layer: Some(SceneLayer::Overlay),
                            node: collider(0.0),
                        },
                        SceneBatchOperation::SetNode {
                            node: SceneBatchNodeRef::Alias("transient".to_owned()),
                            changes: SceneBatchNodeChanges {
                                properties: Some(NodePropertiesPatch {
                                    x: Some(30.0),
                                    ..NodePropertiesPatch::default()
                                }),
                                ..SceneBatchNodeChanges::default()
                            },
                        },
                    ],
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            ClpSceneCommandResult::SceneBatch(batch) => batch,
            _ => unreachable!(),
        };
        let transient = batch.nodes.get("transient").unwrap().node;
        assert!(resources.drain_overlap_events().is_empty());
        assert_eq!(
            resources
                .execute_clp(
                    session,
                    ClpSceneCommand::IsOverlapping { node: anchor },
                    &mut scene,
                    &mut textures,
                )
                .unwrap(),
            ClpSceneCommandResult::IsOverlapping(false)
        );

        resources
            .execute_clp(
                session,
                ClpSceneCommand::SceneBatch {
                    operations: vec![SceneBatchOperation::SetNode {
                        node: SceneBatchNodeRef::Handle(transient),
                        changes: SceneBatchNodeChanges {
                            properties: Some(NodePropertiesPatch {
                                x: Some(5.0),
                                ..NodePropertiesPatch::default()
                            }),
                            ..SceneBatchNodeChanges::default()
                        },
                    }],
                },
                &mut scene,
                &mut textures,
            )
            .unwrap();
        assert!(matches!(
            resources.drain_overlap_events().as_slice(),
            [SceneEvent::OverlapEnter { .. }]
        ));
    }

    #[test]
    fn clp_window_metadata_uses_owned_static_icons_and_keeps_them_alive() {
        let session = session();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let image = match resources
            .execute_clp(
                session,
                ClpSceneCommand::UploadImage {
                    image: ImageUpload::Rgba8 {
                        width: 2,
                        height: 2,
                        pixels: vec![
                            255, 127, 0, 255, 255, 127, 0, 255, 255, 127, 0, 255, 255, 127, 0, 255,
                        ],
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            ClpSceneCommandResult::Image(image) => image,
            _ => unreachable!(),
        };

        assert_eq!(
            resources
                .execute_clp_with_window_icon_support(
                    session,
                    ClpSceneCommand::SetWindowMetadata {
                        metadata: WindowMetadataPatch {
                            title: crate::window_metadata::MetadataField::Set(
                                "Metadata probe".to_owned(),
                            ),
                            icon: crate::window_metadata::MetadataField::Set(image),
                        },
                    },
                    &mut scene,
                    &mut textures,
                    true,
                )
                .unwrap(),
            ClpSceneCommandResult::WindowMetadata {
                icon_supported: true
            }
        );
        assert_eq!(
            resources
                .active_window_metadata()
                .and_then(WindowMetadata::title),
            Some("Metadata probe")
        );
        assert_eq!(
            resources.active_window_icon(&textures).map(|icon| (
                icon.width,
                icon.height,
                icon.pixels.len()
            )),
            Some((2, 2, 16))
        );
        assert_eq!(
            resources.execute_clp(
                session,
                ClpSceneCommand::RemoveImage { image },
                &mut scene,
                &mut textures,
            ),
            Err(ProgramSceneError::ImageInUse(image))
        );

        resources
            .execute_clp(
                session,
                ClpSceneCommand::SetWindowMetadata {
                    metadata: WindowMetadataPatch {
                        title: crate::window_metadata::MetadataField::Clear,
                        icon: crate::window_metadata::MetadataField::Clear,
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap();
        assert!(resources.active_window_metadata().is_none());
        assert_eq!(
            resources
                .execute_clp(
                    session,
                    ClpSceneCommand::RemoveImage { image },
                    &mut scene,
                    &mut textures,
                )
                .unwrap(),
            ClpSceneCommandResult::Acknowledged
        );
    }

    #[test]
    fn clp_window_metadata_rejects_foreign_animated_and_oversized_icons() {
        let (owner, other) = session_pair();
        let mut resources = ProgramSceneResources::new(u32::MAX);
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        let image = match resources
            .execute_clp(
                owner,
                ClpSceneCommand::UploadImage {
                    image: ImageUpload::Rgba8 {
                        width: 1,
                        height: 1,
                        pixels: vec![0, 0, 0, 255],
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            ClpSceneCommandResult::Image(image) => image,
            _ => unreachable!(),
        };
        let metadata_for = |icon| ClpSceneCommand::SetWindowMetadata {
            metadata: WindowMetadataPatch {
                icon: crate::window_metadata::MetadataField::Set(icon),
                ..WindowMetadataPatch::default()
            },
        };
        assert_eq!(
            resources.execute_clp(other, metadata_for(image), &mut scene, &mut textures),
            Err(ProgramSceneError::HandleNotOwned)
        );

        let animated = ImageHandle::new(99);
        resources.images.insert(
            animated,
            OwnedImage {
                session: owner,
                resource: ImageResource::AnimatedImage(AnimatedImageHandle::new(1)),
            },
        );
        assert_eq!(
            resources.execute_clp(owner, metadata_for(animated), &mut scene, &mut textures),
            Err(ProgramSceneError::WindowIconMustBeStaticRgba8(animated))
        );

        let oversized = match resources
            .execute_clp(
                owner,
                ClpSceneCommand::UploadImage {
                    image: ImageUpload::Rgba8 {
                        width: MAX_WINDOW_ICON_DIMENSION + 1,
                        height: 1,
                        pixels: vec![0; (MAX_WINDOW_ICON_DIMENSION as usize + 1) * 4],
                    },
                },
                &mut scene,
                &mut textures,
            )
            .unwrap()
        {
            ClpSceneCommandResult::Image(image) => image,
            _ => unreachable!(),
        };
        assert_eq!(
            resources.execute_clp(owner, metadata_for(oversized), &mut scene, &mut textures),
            Err(ProgramSceneError::WindowIconDimensionLimitExceeded {
                limit: MAX_WINDOW_ICON_DIMENSION
            })
        );
    }
}

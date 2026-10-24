//! Public retained-scene handles, commands, patches, and error model.
//!
//! These types describe the host-internal scene contract consumed by the CLP
//! transport and renderer. Resource ownership and command execution remain in
//! the parent module so this file stays free of retained-state mutation.

use std::{
    collections::BTreeMap,
    error::Error,
    fmt::{Display, Formatter},
};

use crate::{
    animated_image::{AnimatedImageError, AnimatedImageFormat},
    resources::TextureError,
    scene::{ImageContactShape, NodeProperties, Rect, SceneLayer, Transform},
    scene_text::{TextLayoutError, TextMetrics, TextStyle},
    window_metadata::{WindowMetadataError, WindowMetadataPatch},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeHandle(pub(super) u64);

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
pub struct TextureHandle(pub(super) u64);

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
pub struct AnimatedImageHandle(pub(super) u64);

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
pub struct ImageHandle(pub(super) u64);

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
    pub(super) fn into_scene(
        self,
        metrics: TextMetrics,
    ) -> Result<NodeProperties, ProgramSceneError> {
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
    pub(super) fn apply(
        self,
        properties: NodeProperties,
    ) -> Result<NodeProperties, ProgramSceneError> {
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

    pub(super) fn apply(
        self,
        properties: NodeProperties,
    ) -> Result<NodeProperties, ProgramSceneError> {
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
    pub(super) fn created_alias(&self) -> Option<&str> {
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

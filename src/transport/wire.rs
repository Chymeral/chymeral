// Private serde schema and conversion boundary for CLP messages.
//
// The transport facade owns connection lifecycle and scheduling. This source
// file owns only the versioned JSON representation and conversion to the host
// model, so protocol-shape changes remain reviewable in one place.

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireClientMessage {
    Hello {
        protocol: WireProtocolVersion,
        token: String,
        capabilities: Vec<WireCapability>,
    },
    InitComplete,
    Started,
    Fault {
        phase: WireProgramPhase,
        message: String,
    },
    Close,
    Command {
        request_id: String,
        command: Box<WireSceneCommand>,
    },
    PresentScene {
        presentation_id: String,
        updates: Vec<WirePresentationUpdate>,
    },
    Subscribe {
        subscriptions: Vec<WireSubscription>,
    },
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum WireSceneCommand {
    SceneBatch {
        operations: Vec<WireSceneBatchOperation>,
    },
    CreateNode {
        #[serde(default)]
        parent: Option<String>,
        #[serde(default)]
        layer: Option<WireSceneLayer>,
        node: WireNodeInput,
    },
    SetNode {
        node: String,
        changes: WireNodeChanges,
    },
    RemoveNode {
        node: String,
    },
    UploadImage {
        source: WireImageSource,
    },
    RemoveImage {
        image: String,
    },
    IsOverlapping {
        node: String,
    },
    GetOverlaps {
        node: String,
    },
    SetWindowMetadata {
        #[serde(default, skip_serializing_if = "WireMetadataField::is_preserve")]
        title: WireMetadataField<String>,
        #[serde(default, skip_serializing_if = "WireMetadataField::is_preserve")]
        icon: WireMetadataField<String>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum WireMetadataField<T> {
    #[default]
    Preserve,
    Clear,
    Set(T),
}

impl<T> WireMetadataField<T> {
    const fn is_preserve(&self) -> bool {
        matches!(self, Self::Preserve)
    }
}

impl<T> Serialize for WireMetadataField<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Preserve | Self::Clear => serializer.serialize_none(),
            Self::Set(value) => value.serialize(serializer),
        }
    }
}

impl<'de, T> Deserialize<'de> for WireMetadataField<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match Option::<T>::deserialize(deserializer)? {
            Some(value) => Self::Set(value),
            None => Self::Clear,
        })
    }
}

impl WireSceneCommand {
    const fn is_scene_batch(&self) -> bool {
        matches!(self, Self::SceneBatch { .. })
    }

    fn upload_length(&self) -> Result<Option<usize>> {
        match self {
            Self::UploadImage { source } => source.upload_length().map(Some),
            _ => Ok(None),
        }
    }

    fn into_clp_scene_command(self, pixels: Option<Vec<u8>>) -> Result<ClpSceneCommand> {
        match self {
            Self::SceneBatch { operations } => Ok(ClpSceneCommand::SceneBatch {
                operations: operations
                    .into_iter()
                    .map(WireSceneBatchOperation::into_input)
                    .collect::<Result<Vec<_>>>()?,
            }),
            Self::CreateNode {
                parent,
                layer,
                node,
            } => Ok(ClpSceneCommand::CreateNode {
                parent: parent.as_deref().map(parse_node_handle).transpose()?,
                layer: layer.map(WireSceneLayer::into_scene),
                node: node.into_input()?,
            }),
            Self::SetNode { node, changes } => Ok(ClpSceneCommand::SetNode {
                node: parse_node_handle(&node)?,
                changes: changes.into_input()?,
            }),
            Self::RemoveNode { node } => Ok(ClpSceneCommand::RemoveNode {
                node: parse_node_handle(&node)?,
            }),
            Self::UploadImage { source } => Ok(ClpSceneCommand::UploadImage {
                image: source.into_input(
                    pixels.ok_or_else(|| anyhow!("an image upload requires binary image bytes"))?,
                )?,
            }),
            Self::RemoveImage { image } => Ok(ClpSceneCommand::RemoveImage {
                image: parse_image_handle(&image)?,
            }),
            Self::IsOverlapping { node } => Ok(ClpSceneCommand::IsOverlapping {
                node: parse_node_handle(&node)?,
            }),
            Self::GetOverlaps { node } => Ok(ClpSceneCommand::GetOverlaps {
                node: parse_node_handle(&node)?,
            }),
            Self::SetWindowMetadata { title, icon } => Ok(ClpSceneCommand::SetWindowMetadata {
                metadata: WindowMetadataPatch {
                    title: match title {
                        WireMetadataField::Preserve => MetadataField::Preserve,
                        WireMetadataField::Clear => MetadataField::Clear,
                        WireMetadataField::Set(title) => MetadataField::Set(title),
                    },
                    icon: match icon {
                        WireMetadataField::Preserve => MetadataField::Preserve,
                        WireMetadataField::Clear => MetadataField::Clear,
                        WireMetadataField::Set(icon) => {
                            MetadataField::Set(parse_image_handle(&icon)?)
                        }
                    },
                },
            }),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
enum WireSceneBatchNodeRef {
    Handle(String),
    Alias { alias: String },
}

impl WireSceneBatchNodeRef {
    fn into_input(self) -> Result<SceneBatchNodeRef> {
        match self {
            Self::Handle(handle) => Ok(SceneBatchNodeRef::Handle(parse_node_handle(&handle)?)),
            Self::Alias { alias } => Ok(SceneBatchNodeRef::Alias(alias)),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireSceneBatchNodeParent {
    Root,
    Node { node: WireSceneBatchNodeRef },
}

impl WireSceneBatchNodeParent {
    fn into_input(self) -> Result<SceneBatchNodeParent> {
        match self {
            Self::Root => Ok(SceneBatchNodeParent::Root),
            Self::Node { node } => Ok(SceneBatchNodeParent::Node(node.into_input()?)),
        }
    }
}

#[derive(Default, Deserialize, Serialize)]
struct WireSceneBatchNodeChanges {
    #[serde(default)]
    properties: Option<WireNodePropertiesPatch>,
    #[serde(default)]
    parent: Option<WireSceneBatchNodeParent>,
    #[serde(default)]
    layer: Option<WireSceneLayer>,
    #[serde(default)]
    visual: Option<WireNodeVisual>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    text_style: Option<WireTextStyle>,
}

impl WireSceneBatchNodeChanges {
    fn into_input(self) -> Result<SceneBatchNodeChanges> {
        Ok(SceneBatchNodeChanges {
            properties: self.properties.map(WireNodePropertiesPatch::into_patch),
            parent: self
                .parent
                .map(WireSceneBatchNodeParent::into_input)
                .transpose()?,
            layer: self.layer.map(WireSceneLayer::into_scene),
            visual: self.visual.map(WireNodeVisual::into_input).transpose()?,
            text: self.text,
            text_style: self.text_style.map(WireTextStyle::into_scene),
        })
    }

    fn uses_image(&self) -> bool {
        self.visual.as_ref().is_some_and(WireNodeVisual::uses_image)
    }

    const fn uses_text(&self) -> bool {
        self.text.is_some() || self.text_style.is_some()
    }

    fn uses_overlap(&self) -> bool {
        self.properties
            .as_ref()
            .is_some_and(|properties| properties.overlap_enabled.is_some())
    }

    fn uses_external_drop(&self) -> bool {
        self.properties
            .as_ref()
            .is_some_and(|properties| properties.external_drop_enabled.is_some())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum WireSceneBatchOperation {
    #[serde(rename = "create_node")]
    Create {
        alias: String,
        #[serde(default)]
        parent: Option<WireSceneBatchNodeRef>,
        #[serde(default)]
        layer: Option<WireSceneLayer>,
        node: WireNodeInput,
    },
    #[serde(rename = "set_node")]
    Set {
        node: WireSceneBatchNodeRef,
        changes: WireSceneBatchNodeChanges,
    },
    #[serde(rename = "remove_node")]
    Remove { node: WireSceneBatchNodeRef },
}

impl WireSceneBatchOperation {
    fn into_input(self) -> Result<SceneBatchOperation> {
        match self {
            Self::Create {
                alias,
                parent,
                layer,
                node,
            } => Ok(SceneBatchOperation::CreateNode {
                alias,
                parent: parent.map(WireSceneBatchNodeRef::into_input).transpose()?,
                layer: layer.map(WireSceneLayer::into_scene),
                node: node.into_input()?,
            }),
            Self::Set { node, changes } => Ok(SceneBatchOperation::SetNode {
                node: node.into_input()?,
                changes: changes.into_input()?,
            }),
            Self::Remove { node } => Ok(SceneBatchOperation::RemoveNode {
                node: node.into_input()?,
            }),
        }
    }

    fn uses_image(&self) -> bool {
        match self {
            Self::Create { node, .. } => node.uses_image(),
            Self::Set { changes, .. } => changes.uses_image(),
            Self::Remove { .. } => false,
        }
    }

    const fn uses_text(&self) -> bool {
        match self {
            Self::Create { node, .. } => node.uses_text(),
            Self::Set { changes, .. } => changes.uses_text(),
            Self::Remove { .. } => false,
        }
    }

    fn uses_overlap(&self) -> bool {
        match self {
            Self::Create { node, .. } => node.uses_overlap(),
            Self::Set { changes, .. } => changes.uses_overlap(),
            Self::Remove { .. } => false,
        }
    }

    fn uses_external_drop(&self) -> bool {
        match self {
            Self::Create { node, .. } => node.uses_external_drop(),
            Self::Set { changes, .. } => changes.uses_external_drop(),
            Self::Remove { .. } => false,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireImageSource {
    Rgba8 { width: u32, height: u32 },
    Gif { byte_length: usize },
}

impl WireImageSource {
    fn upload_length(&self) -> Result<usize> {
        match self {
            Self::Rgba8 { width, height } => {
                let bytes =
                    rgba8_byte_len(*width, *height).context("invalid RGBA8 image dimensions")?;
                if bytes > MAX_TEXTURE_BYTES {
                    return Err(anyhow!(
                        "an RGBA8 image may not exceed {MAX_TEXTURE_BYTES} bytes"
                    ));
                }
                Ok(bytes)
            }
            Self::Gif { byte_length } => {
                if *byte_length > MAX_ANIMATED_IMAGE_ENCODED_BYTES {
                    return Err(anyhow!(
                        "an encoded GIF image may not exceed {MAX_ANIMATED_IMAGE_ENCODED_BYTES} bytes"
                    ));
                }
                Ok(*byte_length)
            }
        }
    }

    fn into_input(self, bytes: Vec<u8>) -> Result<ImageUpload> {
        match self {
            Self::Rgba8 { width, height } => Ok(ImageUpload::Rgba8 {
                width,
                height,
                pixels: bytes,
            }),
            Self::Gif { .. } => Ok(ImageUpload::Gif { bytes }),
        }
    }
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireSceneLayer {
    Underlay,
    #[default]
    Overlay,
}

impl WireSceneLayer {
    const fn into_scene(self) -> SceneLayer {
        match self {
            Self::Underlay => SceneLayer::Underlay,
            Self::Overlay => SceneLayer::Overlay,
        }
    }
}

#[derive(Deserialize, Serialize)]
struct WireNodeProperties {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    #[serde(default = "default_visible")]
    visible: bool,
    #[serde(default)]
    clip_children: bool,
    #[serde(default)]
    z_index: i32,
    #[serde(default)]
    overlap_enabled: bool,
    #[serde(default)]
    external_drop_enabled: bool,
    #[serde(default)]
    input_passthrough: bool,
}

const fn default_visible() -> bool {
    true
}

impl WireNodeProperties {
    fn into_input(self) -> NodePropertiesInput {
        NodePropertiesInput {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
            visible: self.visible,
            clip_children: self.clip_children,
            z_index: self.z_index,
            overlap_enabled: self.overlap_enabled,
            external_drop_enabled: self.external_drop_enabled,
            input_passthrough: self.input_passthrough,
        }
    }
}

#[derive(Deserialize, Serialize)]
struct WireNodePropertiesPatch {
    x: Option<f64>,
    y: Option<f64>,
    width: Option<f64>,
    height: Option<f64>,
    visible: Option<bool>,
    clip_children: Option<bool>,
    z_index: Option<i32>,
    overlap_enabled: Option<bool>,
    external_drop_enabled: Option<bool>,
    input_passthrough: Option<bool>,
}

impl WireNodePropertiesPatch {
    fn into_patch(self) -> NodePropertiesPatch {
        NodePropertiesPatch {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
            visible: self.visible,
            clip_children: self.clip_children,
            z_index: self.z_index,
            overlap_enabled: self.overlap_enabled,
            external_drop_enabled: self.external_drop_enabled,
            input_passthrough: self.input_passthrough,
        }
    }
}

#[derive(Default, Deserialize, Serialize)]
struct WirePresentationPropertiesPatch {
    #[serde(default)]
    x: Option<f64>,
    #[serde(default)]
    y: Option<f64>,
    #[serde(default)]
    width: Option<f64>,
    #[serde(default)]
    height: Option<f64>,
}

impl WirePresentationPropertiesPatch {
    const fn into_patch(self) -> PresentationPropertiesPatch {
        PresentationPropertiesPatch {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
    }
}

#[derive(Deserialize, Serialize)]
struct WirePresentationUpdate {
    node: String,
    properties: WirePresentationPropertiesPatch,
}

impl WirePresentationUpdate {
    fn into_update(self) -> Result<PresentationUpdate> {
        Ok(PresentationUpdate {
            node: parse_node_handle(&self.node)?,
            properties: self.properties.into_patch(),
        })
    }
}

fn presentation_packet_from_wire(
    presentation_id: String,
    updates: Vec<WirePresentationUpdate>,
) -> Result<PresentationPacket> {
    if presentation_id.is_empty() {
        return Err(anyhow!(
            "a presentation packet requires a non-empty presentation_id"
        ));
    }
    if presentation_id.len() > MAX_PRESENTATION_ID_BYTES {
        return Err(anyhow!(
            "a presentation_id may not exceed {MAX_PRESENTATION_ID_BYTES} bytes"
        ));
    }
    if updates.is_empty() {
        return Err(anyhow!(
            "a presentation packet requires at least one update"
        ));
    }
    if updates.len() > MAX_PRESENTATION_UPDATES {
        return Err(anyhow!(
            "a presentation packet may update at most {MAX_PRESENTATION_UPDATES} nodes"
        ));
    }
    Ok(PresentationPacket {
        presentation_id,
        updates: updates
            .into_iter()
            .map(WirePresentationUpdate::into_update)
            .collect::<Result<Vec<_>>>()?,
    })
}

#[derive(Deserialize, Serialize)]
struct WireTextNodeProperties {
    x: f64,
    y: f64,
    #[serde(default = "default_visible")]
    visible: bool,
    #[serde(default)]
    clip_children: bool,
    #[serde(default)]
    z_index: i32,
    #[serde(default)]
    overlap_enabled: bool,
    #[serde(default)]
    external_drop_enabled: bool,
    #[serde(default)]
    input_passthrough: bool,
}

impl WireTextNodeProperties {
    fn into_input(self) -> TextNodePropertiesInput {
        TextNodePropertiesInput {
            x: self.x,
            y: self.y,
            visible: self.visible,
            clip_children: self.clip_children,
            z_index: self.z_index,
            overlap_enabled: self.overlap_enabled,
            external_drop_enabled: self.external_drop_enabled,
            input_passthrough: self.input_passthrough,
        }
    }
}

#[derive(Default, Deserialize, Serialize)]
struct WireTextStyle {
    #[serde(default)]
    font: Option<String>,
    #[serde(default)]
    size: Option<f32>,
    #[serde(default)]
    line_height: Option<f32>,
    #[serde(default)]
    color: Option<[u8; 4]>,
    #[serde(default)]
    weight: WireTextWeight,
    #[serde(default)]
    align: WireTextAlign,
}

impl WireTextStyle {
    fn into_scene(self) -> TextStyle {
        TextStyle {
            font_alias: self.font,
            size: self.size,
            line_height: self.line_height,
            color: self.color,
            weight: self.weight.into_scene(),
            align: self.align.into_scene(),
        }
    }
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireTextWeight {
    #[default]
    Normal,
    Bold,
}

impl WireTextWeight {
    const fn into_scene(self) -> TextWeight {
        match self {
            Self::Normal => TextWeight::Normal,
            Self::Bold => TextWeight::Bold,
        }
    }
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireTextAlign {
    #[default]
    Start,
    Center,
    End,
}

impl WireTextAlign {
    const fn into_scene(self) -> TextAlign {
        match self {
            Self::Start => TextAlign::Start,
            Self::Center => TextAlign::Center,
            Self::End => TextAlign::End,
        }
    }
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireImageContactShape {
    #[default]
    Bounds,
    Alpha {
        threshold: u8,
    },
}

impl WireImageContactShape {
    const fn into_scene(self) -> ImageContactShape {
        match self {
            Self::Bounds => ImageContactShape::Bounds,
            Self::Alpha { threshold } => ImageContactShape::Alpha { threshold },
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireNodeInput {
    Empty {
        properties: WireNodeProperties,
    },
    Rectangle {
        properties: WireNodeProperties,
        color: [u8; 4],
    },
    Image {
        properties: WireNodeProperties,
        image: String,
        opacity: f32,
        #[serde(default)]
        contact_shape: WireImageContactShape,
    },
    Text {
        properties: WireTextNodeProperties,
        text: String,
        #[serde(default)]
        style: WireTextStyle,
    },
}

impl WireNodeInput {
    fn into_input(self) -> Result<NodeInput> {
        match self {
            Self::Empty { properties } => Ok(NodeInput::Empty {
                properties: properties.into_input(),
            }),
            Self::Rectangle { properties, color } => Ok(NodeInput::Rectangle {
                properties: properties.into_input(),
                color,
            }),
            Self::Image {
                properties,
                image,
                opacity,
                contact_shape,
            } => Ok(NodeInput::Image {
                properties: properties.into_input(),
                image: parse_image_handle(&image)?,
                opacity,
                contact_shape: contact_shape.into_scene(),
            }),
            Self::Text {
                properties,
                text,
                style,
            } => Ok(NodeInput::Text {
                properties: properties.into_input(),
                text,
                style: style.into_scene(),
            }),
        }
    }

    const fn uses_image(&self) -> bool {
        matches!(self, Self::Image { .. })
    }

    const fn uses_text(&self) -> bool {
        matches!(self, Self::Text { .. })
    }

    fn uses_overlap(&self) -> bool {
        match self {
            Self::Empty { properties }
            | Self::Rectangle { properties, .. }
            | Self::Image { properties, .. } => properties.overlap_enabled,
            Self::Text { properties, .. } => properties.overlap_enabled,
        }
    }

    fn uses_external_drop(&self) -> bool {
        match self {
            Self::Empty { properties }
            | Self::Rectangle { properties, .. }
            | Self::Image { properties, .. } => properties.external_drop_enabled,
            Self::Text { properties, .. } => properties.external_drop_enabled,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireNodeVisual {
    Empty,
    Rectangle {
        color: [u8; 4],
    },
    Image {
        image: String,
        opacity: f32,
        #[serde(default)]
        contact_shape: WireImageContactShape,
    },
}

impl WireNodeVisual {
    fn into_input(self) -> Result<NodeVisualInput> {
        match self {
            Self::Empty => Ok(NodeVisualInput::Empty),
            Self::Rectangle { color } => Ok(NodeVisualInput::Rectangle { color }),
            Self::Image {
                image,
                opacity,
                contact_shape,
            } => Ok(NodeVisualInput::Image {
                image: parse_image_handle(&image)?,
                opacity,
                contact_shape: contact_shape.into_scene(),
            }),
        }
    }

    const fn uses_image(&self) -> bool {
        matches!(self, Self::Image { .. })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireNodeParent {
    Root,
    Node { node: String },
}

impl WireNodeParent {
    fn into_input(self) -> Result<NodeParent> {
        match self {
            Self::Root => Ok(NodeParent::Root),
            Self::Node { node } => Ok(NodeParent::Node(parse_node_handle(&node)?)),
        }
    }
}

#[derive(Default, Deserialize, Serialize)]
struct WireNodeChanges {
    #[serde(default)]
    properties: Option<WireNodePropertiesPatch>,
    #[serde(default)]
    parent: Option<WireNodeParent>,
    #[serde(default)]
    layer: Option<WireSceneLayer>,
    #[serde(default)]
    visual: Option<WireNodeVisual>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    text_style: Option<WireTextStyle>,
}

impl WireNodeChanges {
    fn into_input(self) -> Result<NodeChanges> {
        Ok(NodeChanges {
            properties: self.properties.map(WireNodePropertiesPatch::into_patch),
            parent: self.parent.map(WireNodeParent::into_input).transpose()?,
            layer: self.layer.map(WireSceneLayer::into_scene),
            visual: self.visual.map(WireNodeVisual::into_input).transpose()?,
            text: self.text,
            text_style: self.text_style.map(WireTextStyle::into_scene),
        })
    }

    fn uses_image(&self) -> bool {
        self.visual.as_ref().is_some_and(WireNodeVisual::uses_image)
    }

    const fn uses_text(&self) -> bool {
        self.text.is_some() || self.text_style.is_some()
    }

    fn uses_overlap(&self) -> bool {
        self.properties
            .as_ref()
            .is_some_and(|properties| properties.overlap_enabled.is_some())
    }

    fn uses_external_drop(&self) -> bool {
        self.properties
            .as_ref()
            .is_some_and(|properties| properties.external_drop_enabled.is_some())
    }
}

fn parse_node_handle(value: &str) -> Result<NodeHandle> {
    parse_handle(value, "node-").map(NodeHandle::new)
}

fn parse_image_handle(value: &str) -> Result<ImageHandle> {
    parse_handle(value, "image-").map(ImageHandle::new)
}

fn parse_handle(value: &str, prefix: &str) -> Result<u64> {
    let numeric = value
        .strip_prefix(prefix)
        .ok_or_else(|| anyhow!("invalid resource handle"))?;
    let value = numeric.parse::<u64>().context("invalid resource handle")?;
    if value == 0 {
        return Err(anyhow!("invalid resource handle"));
    }
    Ok(value)
}

struct WireHello {
    protocol: WireProtocolVersion,
    token: String,
    capabilities: Vec<WireCapability>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
struct WireProtocolVersion {
    major: u16,
    minor: u16,
}

impl From<ProtocolVersion> for WireProtocolVersion {
    fn from(version: ProtocolVersion) -> Self {
        Self {
            major: version.major(),
            minor: version.minor(),
        }
    }
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireCapability {
    Scene,
    Images,
    Input,
    Timing,
    Overlap,
    ExternalDrop,
    Text,
    Presentation,
    WindowMetadata,
    SceneBatch,
}

impl WireCapability {
    const fn into_protocol(self) -> Capability {
        match self {
            Self::Scene => Capability::Scene,
            Self::Images => Capability::Images,
            Self::Input => Capability::Input,
            Self::Timing => Capability::Timing,
            Self::Overlap => Capability::Overlap,
            Self::ExternalDrop => Capability::ExternalDrop,
            Self::Text => Capability::Text,
            Self::Presentation => Capability::Presentation,
            Self::WindowMetadata => Capability::WindowMetadata,
            Self::SceneBatch => Capability::SceneBatch,
        }
    }
}

impl From<Capability> for WireCapability {
    fn from(capability: Capability) -> Self {
        match capability {
            Capability::Scene => Self::Scene,
            Capability::Images => Self::Images,
            Capability::Input => Self::Input,
            Capability::Timing => Self::Timing,
            Capability::Overlap => Self::Overlap,
            Capability::ExternalDrop => Self::ExternalDrop,
            Capability::Text => Self::Text,
            Capability::Presentation => Self::Presentation,
            Capability::WindowMetadata => Self::WindowMetadata,
            Capability::SceneBatch => Self::SceneBatch,
        }
    }
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum WireSubscription {
    RawInput,
    Terminal,
    Timing,
}

impl WireSubscription {
    const fn into_subscription(self) -> Subscription {
        match self {
            Self::RawInput => Subscription::RawInput,
            Self::Terminal => Subscription::Terminal,
            Self::Timing => Subscription::Timing,
        }
    }
}

impl From<Subscription> for WireSubscription {
    fn from(subscription: Subscription) -> Self {
        match subscription {
            Subscription::RawInput => Self::RawInput,
            Subscription::Terminal => Self::Terminal,
            Subscription::Timing => Self::Timing,
        }
    }
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireProgramPhase {
    Init,
    Start,
    Event,
    Command,
    Tick,
    Frame,
}

impl WireProgramPhase {
    const fn into_protocol(self) -> ProgramPhase {
        match self {
            Self::Init => ProgramPhase::Init,
            Self::Start => ProgramPhase::Start,
            Self::Event => ProgramPhase::Event,
            Self::Command => ProgramPhase::Command,
            Self::Tick => ProgramPhase::Tick,
            Self::Frame => ProgramPhase::Frame,
        }
    }
}

impl From<ProgramPhase> for WireProgramPhase {
    fn from(phase: ProgramPhase) -> Self {
        match phase {
            ProgramPhase::Init => Self::Init,
            ProgramPhase::Start => Self::Start,
            ProgramPhase::Event => Self::Event,
            ProgramPhase::Command => Self::Command,
            ProgramPhase::Tick => Self::Tick,
            ProgramPhase::Frame => Self::Frame,
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireHostMessage {
    Init {
        protocol: WireProtocolVersion,
        session_id: String,
        capabilities: Vec<WireCapability>,
        context: WireInitContext,
    },
    Start {
        session_id: String,
    },
    Event {
        session_id: String,
        event: WireEvent,
    },
    Tick {
        session_id: String,
        dt_ns: String,
    },
    Frame {
        session_id: String,
        dt_ns: String,
    },
    Resize {
        session_id: String,
        physical_width: u32,
        physical_height: u32,
        logical_width: f64,
        logical_height: f64,
        scale_factor: f64,
    },
    Subscribed {
        session_id: String,
        subscriptions: Vec<WireSubscription>,
    },
    DeliveryGap {
        session_id: String,
        categories: Vec<WireDeliveryCategory>,
    },
    PresentationStatus {
        session_id: String,
        presentation_id: String,
        status: WirePresentationStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    Closing {
        session_id: String,
        reason: WireCloseReason,
    },
    Closed {
        session_id: String,
        reason: WireCloseReason,
    },
    Error {
        code: String,
        message: String,
    },
    Result {
        request_id: String,
        result: WireCommandResult,
    },
    CommandError {
        request_id: String,
        code: String,
        message: String,
    },
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum WirePresentationStatus {
    Presented,
    Rejected,
    Superseded,
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireEvent {
    Key {
        key: WireKey,
        text: Option<String>,
        state: WireKeyState,
        modifiers: WireModifiers,
        repeat: bool,
    },
    PointerMove {
        position: WireVec2,
        modifiers: WireModifiers,
    },
    PointerPress {
        button: WirePointerButton,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    PointerRelease {
        button: WirePointerButton,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    Scroll {
        position: WireVec2,
        delta: WireVec2,
        unit: WireScrollUnit,
        modifiers: WireModifiers,
    },
    Focus {
        state: WireFocusState,
    },
    Scene {
        event: WireSceneEvent,
    },
    Terminal {
        state: WireTerminalState,
    },
    Custom {
        name: String,
        payload: Vec<u8>,
    },
}

impl WireEvent {
    fn from_engine_event(event: &EngineEvent) -> Option<Self> {
        match event {
            EngineEvent::Key(event) => Some(Self::Key {
                key: event.key.clone().into(),
                text: event.text.clone(),
                state: event.state.into(),
                modifiers: event.modifiers.into(),
                repeat: event.repeat,
            }),
            EngineEvent::Pointer(PointerEvent::Move {
                position,
                modifiers,
            }) => Some(Self::PointerMove {
                position: (*position).into(),
                modifiers: (*modifiers).into(),
            }),
            EngineEvent::Pointer(PointerEvent::Press {
                button,
                position,
                modifiers,
            }) => Some(Self::PointerPress {
                button: (*button).into(),
                position: (*position).into(),
                modifiers: (*modifiers).into(),
            }),
            EngineEvent::Pointer(PointerEvent::Release {
                button,
                position,
                modifiers,
            }) => Some(Self::PointerRelease {
                button: (*button).into(),
                position: (*position).into(),
                modifiers: (*modifiers).into(),
            }),
            EngineEvent::Scroll(ScrollEvent {
                position,
                delta,
                unit,
                modifiers,
            }) => Some(Self::Scroll {
                position: (*position).into(),
                delta: (*delta).into(),
                unit: (*unit).into(),
                modifiers: (*modifiers).into(),
            }),
            EngineEvent::Focus(event) => Some(Self::Focus {
                state: (*event).into(),
            }),
            EngineEvent::Scene(_) => None,
            EngineEvent::Terminal(event) => Some(Self::Terminal {
                state: event.clone().into(),
            }),
            EngineEvent::Custom(event) => Some(Self::Custom {
                name: event.name.clone(),
                payload: event.payload.clone(),
            }),
        }
    }

    const fn delivery_category(&self) -> Option<DeliveryCategory> {
        match self {
            Self::PointerMove { .. } => Some(DeliveryCategory::RawInput),
            Self::Terminal {
                state: WireTerminalState::OutputAvailable,
            } => Some(DeliveryCategory::Terminal),
            Self::Scene {
                event:
                    WireSceneEvent::PointerMove { .. }
                    | WireSceneEvent::DragMove { .. }
                    | WireSceneEvent::ExternalDropOver { .. },
            } => Some(DeliveryCategory::Scene),
            Self::Key { .. }
            | Self::PointerPress { .. }
            | Self::PointerRelease { .. }
            | Self::Scroll { .. }
            | Self::Focus { .. }
            | Self::Custom { .. } => Some(DeliveryCategory::RawInput),
            Self::Terminal {
                state: WireTerminalState::Exited,
            } => Some(DeliveryCategory::Terminal),
            Self::Scene { .. } => Some(DeliveryCategory::Scene),
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireDeliveryCategory {
    RawInput,
    Scene,
    Terminal,
    Timing,
}

impl From<DeliveryCategory> for WireDeliveryCategory {
    fn from(category: DeliveryCategory) -> Self {
        match category {
            DeliveryCategory::RawInput => Self::RawInput,
            DeliveryCategory::Scene => Self::Scene,
            DeliveryCategory::Terminal => Self::Terminal,
            DeliveryCategory::Timing => Self::Timing,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireSceneEvent {
    PointerEnter {
        target: String,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    PointerLeave {
        target: String,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    PointerMove {
        target: String,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    PointerPress {
        target: String,
        button: WirePointerButton,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    PointerRelease {
        target: String,
        button: WirePointerButton,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    FocusGained {
        target: String,
    },
    FocusLost {
        target: String,
    },
    DragStart {
        source: String,
        origin: WireVec2,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    DragEnter {
        source: Option<String>,
        target: Option<String>,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    DragLeave {
        source: Option<String>,
        target: Option<String>,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    DragMove {
        source: Option<String>,
        target: Option<String>,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    Drop {
        source: Option<String>,
        target: Option<String>,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    DragEnd {
        source: Option<String>,
        target: Option<String>,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    DragCancel {
        source: String,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    OverlapEnter {
        first: String,
        second: String,
    },
    OverlapLeave {
        first: String,
        second: String,
    },
    ExternalDropEnter {
        target: String,
        position: WireVec2,
    },
    ExternalDropOver {
        target: String,
        position: WireVec2,
    },
    ExternalDropLeave {
        target: String,
        position: WireVec2,
    },
    ExternalDrop {
        target: String,
        position: WireVec2,
        files: Vec<WireExternalFile>,
    },
}

#[derive(Clone, Serialize)]
struct WireExternalFile {
    uri: String,
}

impl WireSceneEvent {
    fn from_scene_event(
        event: &SceneEvent,
        mut node_handle_for: impl FnMut(NodeId) -> Option<String>,
    ) -> Option<Self> {
        match event {
            SceneEvent::PointerEnter {
                target,
                position,
                modifiers,
            } => Some(Self::PointerEnter {
                target: node_handle_for(*target)?,
                position: (*position).into(),
                modifiers: (*modifiers).into(),
            }),
            SceneEvent::PointerLeave {
                target,
                position,
                modifiers,
            } => Some(Self::PointerLeave {
                target: node_handle_for(*target)?,
                position: (*position).into(),
                modifiers: (*modifiers).into(),
            }),
            SceneEvent::PointerMove {
                target,
                position,
                modifiers,
            } => Some(Self::PointerMove {
                target: node_handle_for(*target)?,
                position: (*position).into(),
                modifiers: (*modifiers).into(),
            }),
            SceneEvent::PointerPress {
                target,
                button,
                position,
                modifiers,
            } => Some(Self::PointerPress {
                target: node_handle_for(*target)?,
                button: (*button).into(),
                position: (*position).into(),
                modifiers: (*modifiers).into(),
            }),
            SceneEvent::PointerRelease {
                target,
                button,
                position,
                modifiers,
            } => Some(Self::PointerRelease {
                target: node_handle_for(*target)?,
                button: (*button).into(),
                position: (*position).into(),
                modifiers: (*modifiers).into(),
            }),
            SceneEvent::FocusGained { target } => Some(Self::FocusGained {
                target: node_handle_for(*target)?,
            }),
            SceneEvent::FocusLost { target } => Some(Self::FocusLost {
                target: node_handle_for(*target)?,
            }),
            SceneEvent::DragStart {
                source,
                origin,
                position,
                modifiers,
            } => Some(Self::DragStart {
                source: node_handle_for(*source)?,
                origin: (*origin).into(),
                position: (*position).into(),
                modifiers: (*modifiers).into(),
            }),
            SceneEvent::DragEnter {
                source,
                target,
                position,
                modifiers,
            } => Self::with_drag_handles(
                node_handle_for(*source),
                node_handle_for(*target),
                |source, target| Self::DragEnter {
                    source,
                    target,
                    position: (*position).into(),
                    modifiers: (*modifiers).into(),
                },
            ),
            SceneEvent::DragLeave {
                source,
                target,
                position,
                modifiers,
            } => Self::with_drag_handles(
                node_handle_for(*source),
                node_handle_for(*target),
                |source, target| Self::DragLeave {
                    source,
                    target,
                    position: (*position).into(),
                    modifiers: (*modifiers).into(),
                },
            ),
            SceneEvent::DragMove {
                source,
                target,
                position,
                modifiers,
            } => Self::with_drag_handles(
                node_handle_for(*source),
                target.and_then(&mut node_handle_for),
                |source, target| Self::DragMove {
                    source,
                    target,
                    position: (*position).into(),
                    modifiers: (*modifiers).into(),
                },
            ),
            SceneEvent::Drop {
                source,
                target,
                position,
                modifiers,
            } => Self::with_drag_handles(
                node_handle_for(*source),
                node_handle_for(*target),
                |source, target| Self::Drop {
                    source,
                    target,
                    position: (*position).into(),
                    modifiers: (*modifiers).into(),
                },
            ),
            SceneEvent::DragEnd {
                source,
                target,
                position,
                modifiers,
            } => Self::with_drag_handles(
                node_handle_for(*source),
                target.and_then(&mut node_handle_for),
                |source, target| Self::DragEnd {
                    source,
                    target,
                    position: (*position).into(),
                    modifiers: (*modifiers).into(),
                },
            ),
            SceneEvent::DragCancel {
                source,
                position,
                modifiers,
            } => Some(Self::DragCancel {
                source: node_handle_for(*source)?,
                position: (*position).into(),
                modifiers: (*modifiers).into(),
            }),
            SceneEvent::OverlapEnter { first, second } => Some(Self::OverlapEnter {
                first: node_handle_for(*first)?,
                second: node_handle_for(*second)?,
            }),
            SceneEvent::OverlapLeave { first, second } => Some(Self::OverlapLeave {
                first: node_handle_for(*first)?,
                second: node_handle_for(*second)?,
            }),
            SceneEvent::ExternalDropEnter { target, position } => Some(Self::ExternalDropEnter {
                target: node_handle_for(*target)?,
                position: (*position).into(),
            }),
            SceneEvent::ExternalDropOver { target, position } => Some(Self::ExternalDropOver {
                target: node_handle_for(*target)?,
                position: (*position).into(),
            }),
            SceneEvent::ExternalDropLeave { target, position } => Some(Self::ExternalDropLeave {
                target: node_handle_for(*target)?,
                position: (*position).into(),
            }),
            SceneEvent::ExternalDrop {
                target,
                position,
                files,
            } => Some(Self::ExternalDrop {
                target: node_handle_for(*target)?,
                position: (*position).into(),
                files: files
                    .iter()
                    .map(|file| WireExternalFile {
                        uri: file.uri.clone(),
                    })
                    .collect(),
            }),
        }
    }

    const fn required_capability(&self) -> Capability {
        match self {
            Self::OverlapEnter { .. } | Self::OverlapLeave { .. } => Capability::Overlap,
            Self::ExternalDropEnter { .. }
            | Self::ExternalDropOver { .. }
            | Self::ExternalDropLeave { .. }
            | Self::ExternalDrop { .. } => Capability::ExternalDrop,
            Self::PointerEnter { .. }
            | Self::PointerLeave { .. }
            | Self::PointerMove { .. }
            | Self::PointerPress { .. }
            | Self::PointerRelease { .. }
            | Self::FocusGained { .. }
            | Self::FocusLost { .. }
            | Self::DragStart { .. }
            | Self::DragEnter { .. }
            | Self::DragLeave { .. }
            | Self::DragMove { .. }
            | Self::Drop { .. }
            | Self::DragEnd { .. }
            | Self::DragCancel { .. } => Capability::Input,
        }
    }

    fn with_drag_handles(
        source: Option<String>,
        target: Option<String>,
        build: impl FnOnce(Option<String>, Option<String>) -> Self,
    ) -> Option<Self> {
        (source.is_some() || target.is_some()).then(|| build(source, target))
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireTerminalState {
    OutputAvailable,
    Exited,
}

impl From<TerminalEvent> for WireTerminalState {
    fn from(event: TerminalEvent) -> Self {
        match event {
            TerminalEvent::OutputAvailable => Self::OutputAvailable,
            TerminalEvent::Exited => Self::Exited,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireKey {
    Character { value: String },
    Named { value: String },
    Dead { value: Option<String> },
    Unidentified,
}

impl From<Key> for WireKey {
    fn from(key: Key) -> Self {
        match key {
            Key::Character(value) => Self::Character { value },
            Key::Named(key) => Self::Named {
                value: wire_named_key(key),
            },
            Key::Dead(value) => Self::Dead {
                value: value.map(|character| character.to_string()),
            },
            Key::Unidentified => Self::Unidentified,
        }
    }
}

fn wire_named_key(key: NamedKey) -> String {
    match key {
        NamedKey::Alt => "alt".to_owned(),
        NamedKey::Backspace => "backspace".to_owned(),
        NamedKey::Delete => "delete".to_owned(),
        NamedKey::End => "end".to_owned(),
        NamedKey::Enter => "enter".to_owned(),
        NamedKey::Escape => "escape".to_owned(),
        NamedKey::Home => "home".to_owned(),
        NamedKey::Insert => "insert".to_owned(),
        NamedKey::PageDown => "page_down".to_owned(),
        NamedKey::PageUp => "page_up".to_owned(),
        NamedKey::Space => "space".to_owned(),
        NamedKey::Tab => "tab".to_owned(),
        NamedKey::ArrowDown => "arrow_down".to_owned(),
        NamedKey::ArrowLeft => "arrow_left".to_owned(),
        NamedKey::ArrowRight => "arrow_right".to_owned(),
        NamedKey::ArrowUp => "arrow_up".to_owned(),
        NamedKey::Other(value) => value,
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireKeyState {
    Pressed,
    Released,
}

impl From<KeyState> for WireKeyState {
    fn from(state: KeyState) -> Self {
        match state {
            KeyState::Pressed => Self::Pressed,
            KeyState::Released => Self::Released,
        }
    }
}

#[derive(Clone, Copy, Serialize)]
struct WireModifiers {
    shift: bool,
    control: bool,
    alt: bool,
    super_key: bool,
}

impl From<Modifiers> for WireModifiers {
    fn from(modifiers: Modifiers) -> Self {
        Self {
            shift: modifiers.shift,
            control: modifiers.control,
            alt: modifiers.alt,
            super_key: modifiers.super_key,
        }
    }
}

#[derive(Clone, Copy, Serialize)]
struct WireVec2 {
    x: f64,
    y: f64,
}

impl From<Vec2> for WireVec2 {
    fn from(position: Vec2) -> Self {
        Self {
            x: position.x,
            y: position.y,
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WirePointerButton {
    Primary,
    Secondary,
    Middle,
    Back,
    Forward,
    Other { value: u16 },
}

impl From<PointerButton> for WirePointerButton {
    fn from(button: PointerButton) -> Self {
        match button {
            PointerButton::Primary => Self::Primary,
            PointerButton::Secondary => Self::Secondary,
            PointerButton::Middle => Self::Middle,
            PointerButton::Back => Self::Back,
            PointerButton::Forward => Self::Forward,
            PointerButton::Other(value) => Self::Other { value },
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireScrollUnit {
    Line,
    Pixel,
}

impl From<ScrollUnit> for WireScrollUnit {
    fn from(unit: ScrollUnit) -> Self {
        match unit {
            ScrollUnit::Line => Self::Line,
            ScrollUnit::Pixel => Self::Pixel,
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireFocusState {
    Gained,
    Lost,
}

impl From<FocusEvent> for WireFocusState {
    fn from(event: FocusEvent) -> Self {
        match event {
            FocusEvent::Gained => Self::Gained,
            FocusEvent::Lost => Self::Lost,
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireCommandResult {
    SceneBatch {
        nodes: BTreeMap<String, WireSceneBatchCreatedNode>,
        operations: Vec<WireSceneBatchOperationResult>,
    },
    Node {
        node: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        text_metrics: Option<WireTextMetrics>,
    },
    NodeUpdated {
        #[serde(skip_serializing_if = "Option::is_none")]
        text_metrics: Option<WireTextMetrics>,
    },
    Image {
        image: String,
    },
    IsOverlapping {
        overlapping: bool,
    },
    Overlaps {
        nodes: Vec<String>,
    },
    WindowMetadata {
        icon_supported: bool,
    },
    Acknowledged,
}

#[derive(Serialize)]
struct WireSceneBatchCreatedNode {
    node: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text_metrics: Option<WireTextMetrics>,
}

impl From<SceneBatchCreatedNode> for WireSceneBatchCreatedNode {
    fn from(node: SceneBatchCreatedNode) -> Self {
        Self {
            node: node.node.to_string(),
            text_metrics: node.text_metrics.map(Into::into),
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireSceneBatchOperationResult {
    Created {
        alias: String,
    },
    Updated {
        #[serde(skip_serializing_if = "Option::is_none")]
        text_metrics: Option<WireTextMetrics>,
    },
    Removed,
}

impl From<SceneBatchOperationResult> for WireSceneBatchOperationResult {
    fn from(result: SceneBatchOperationResult) -> Self {
        match result {
            SceneBatchOperationResult::Created { alias } => Self::Created { alias },
            SceneBatchOperationResult::Updated { text_metrics } => Self::Updated {
                text_metrics: text_metrics.map(Into::into),
            },
            SceneBatchOperationResult::Removed => Self::Removed,
        }
    }
}

impl From<ClpSceneCommandResult> for WireCommandResult {
    fn from(result: ClpSceneCommandResult) -> Self {
        match result {
            ClpSceneCommandResult::SceneBatch(batch) => Self::SceneBatch {
                nodes: batch
                    .nodes
                    .into_iter()
                    .map(|(alias, node)| (alias, node.into()))
                    .collect(),
                operations: batch.operations.into_iter().map(Into::into).collect(),
            },
            ClpSceneCommandResult::Node { node, text_metrics } => Self::Node {
                node: node.to_string(),
                text_metrics: text_metrics.map(Into::into),
            },
            ClpSceneCommandResult::NodeUpdated { text_metrics } => Self::NodeUpdated {
                text_metrics: text_metrics.map(Into::into),
            },
            ClpSceneCommandResult::Image(image) => Self::Image {
                image: image.to_string(),
            },
            ClpSceneCommandResult::IsOverlapping(overlapping) => {
                Self::IsOverlapping { overlapping }
            }
            ClpSceneCommandResult::Overlaps(nodes) => Self::Overlaps {
                nodes: nodes.into_iter().map(|node| node.to_string()).collect(),
            },
            ClpSceneCommandResult::WindowMetadata { icon_supported } => {
                Self::WindowMetadata { icon_supported }
            }
            ClpSceneCommandResult::Acknowledged => Self::Acknowledged,
        }
    }
}

#[derive(Serialize)]
struct WireTextMetrics {
    columns: u32,
    rows: u32,
    width: f64,
    height: f64,
}

impl From<TextMetrics> for WireTextMetrics {
    fn from(metrics: TextMetrics) -> Self {
        Self {
            columns: metrics.columns,
            rows: metrics.rows,
            width: metrics.width,
            height: metrics.height,
        }
    }
}

impl From<HostMessage> for WireHostMessage {
    fn from(message: HostMessage) -> Self {
        match message {
            HostMessage::Init(init) => Self::Init {
                protocol: init.protocol_version.into(),
                session_id: init.session_id.to_string(),
                capabilities: capabilities_to_wire(init.granted_capabilities),
                context: WireInitContext::from_context(init.context, init.granted_capabilities),
            },
            HostMessage::Start { session_id } => Self::Start {
                session_id: session_id.to_string(),
            },
            HostMessage::Closed { session_id, reason } => Self::Closed {
                session_id: session_id.to_string(),
                reason: reason.into(),
            },
        }
    }
}

impl WireHostMessage {
    fn from_notification(session_id: SessionId, notification: HostNotification) -> Self {
        let session_id = session_id.to_string();
        match notification {
            HostNotification::Event(event) => Self::Event { session_id, event },
            HostNotification::Tick { dt_ns } => Self::Tick { session_id, dt_ns },
            HostNotification::Frame { dt_ns } => Self::Frame { session_id, dt_ns },
            HostNotification::Resize {
                physical_width,
                physical_height,
                logical_width,
                logical_height,
                scale_factor,
            } => Self::Resize {
                session_id,
                physical_width,
                physical_height,
                logical_width,
                logical_height,
                scale_factor,
            },
            HostNotification::PresentationStatus {
                presentation_id,
                status,
            } => {
                let (status, code, message) = match status {
                    PresentationStatus::Presented => {
                        (WirePresentationStatus::Presented, None, None)
                    }
                    PresentationStatus::Rejected { code, message } => {
                        (WirePresentationStatus::Rejected, Some(code), Some(message))
                    }
                    PresentationStatus::Superseded => {
                        (WirePresentationStatus::Superseded, None, None)
                    }
                };
                Self::PresentationStatus {
                    session_id,
                    presentation_id,
                    status,
                    code,
                    message,
                }
            }
            HostNotification::DeliveryGap { categories } => Self::DeliveryGap {
                session_id,
                categories: categories
                    .into_iter()
                    .map(WireDeliveryCategory::from)
                    .collect(),
            },
        }
    }
}

fn capabilities_to_wire(capabilities: CapabilitySet) -> Vec<WireCapability> {
    [
        Capability::Scene,
        Capability::Images,
        Capability::Input,
        Capability::Timing,
        Capability::Overlap,
        Capability::ExternalDrop,
        Capability::Text,
        Capability::Presentation,
        Capability::WindowMetadata,
        Capability::SceneBatch,
    ]
    .into_iter()
    .filter(|capability| capabilities.contains(*capability))
    .map(WireCapability::from)
    .collect()
}

#[derive(Serialize)]
struct WireInitContext {
    arguments: Vec<String>,
    initial_commands: Vec<WireStartupCommand>,
    #[serde(skip_serializing_if = "Option::is_none")]
    window_metadata: Option<WireWindowMetadataSupport>,
}

#[derive(Serialize)]
struct WireWindowMetadataSupport {
    icon_supported: bool,
}

impl WireInitContext {
    fn from_context(context: ProgramInitContext, capabilities: CapabilitySet) -> Self {
        Self {
            arguments: context.arguments,
            initial_commands: context
                .initial_commands
                .into_iter()
                .map(|command| WireStartupCommand {
                    name: command.name,
                    arguments: command.arguments,
                })
                .collect(),
            window_metadata: capabilities
                .contains(Capability::WindowMetadata)
                .then_some(context.window_metadata)
                .flatten()
                .map(|support| WireWindowMetadataSupport {
                    icon_supported: support.icon_supported,
                }),
        }
    }
}

#[derive(Serialize)]
struct WireStartupCommand {
    name: String,
    arguments: Vec<String>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireCloseReason {
    ClientRequested,
    ProgramFault {
        phase: WireProgramPhase,
        message: String,
    },
    HostShutdown,
    HostOverloaded,
    ProtocolViolation,
    TransportDisconnected,
}

impl From<SessionCloseReason> for WireCloseReason {
    fn from(reason: SessionCloseReason) -> Self {
        match reason {
            SessionCloseReason::ClientRequested => Self::ClientRequested,
            SessionCloseReason::ProgramFault(fault) => Self::ProgramFault {
                phase: fault.phase.into(),
                message: fault.message,
            },
            SessionCloseReason::HostShutdown => Self::HostShutdown,
            SessionCloseReason::HostOverloaded => Self::HostOverloaded,
            SessionCloseReason::ProtocolViolation => Self::ProtocolViolation,
            SessionCloseReason::TransportDisconnected => Self::TransportDisconnected,
        }
    }
}

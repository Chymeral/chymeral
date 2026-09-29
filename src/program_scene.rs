use std::{
    collections::BTreeMap,
    error::Error,
    fmt::{Display, Formatter},
};

use crate::{
    protocol::SessionId,
    resources::{Texture, TextureError, TextureStore, rgba8_byte_len},
    scene::{
        ImageNode, NodeContent, NodeId, NodeProperties, Rect, Scene, SceneLayer, SolidNode,
        Transform,
    },
};

pub const MAX_TEXTURE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_SESSION_TEXTURE_BYTES: usize = 128 * 1024 * 1024;

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodePropertiesInput {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub visible: bool,
    pub clip_children: bool,
    pub z_index: i32,
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
        })
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
        }
        .into_scene()
    }
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
    },
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
    Reparent {
        node: NodeHandle,
        parent: Option<NodeHandle>,
    },
    RemoveNode {
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneCommandResult {
    Node(NodeHandle),
    Texture(TextureHandle),
    Acknowledged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgramSceneError {
    InvalidNodeHandle(NodeHandle),
    InvalidTextureHandle(TextureHandle),
    HandleNotOwned,
    TextureInUse(TextureHandle),
    TextureDimensionLimitExceeded { limit: u32 },
    TextureLimitExceeded { limit: usize },
    SessionTextureLimitExceeded { limit: usize },
    Geometry(crate::scene::GeometryError),
    Scene(crate::scene::SceneError),
    Image(crate::scene::ImageNodeError),
    Texture(TextureError),
    HandleExhausted,
}

impl Display for ProgramSceneError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidNodeHandle(handle) => write!(formatter, "{handle} is not valid"),
            Self::InvalidTextureHandle(handle) => write!(formatter, "{handle} is not valid"),
            Self::HandleNotOwned => {
                formatter.write_str("the resource handle is not owned by this session")
            }
            Self::TextureInUse(handle) => {
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
    next_texture_handle: u64,
    nodes: BTreeMap<NodeHandle, OwnedNode>,
    textures: BTreeMap<TextureHandle, OwnedTexture>,
    session_texture_bytes: BTreeMap<SessionId, usize>,
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

impl ProgramSceneResources {
    pub fn new(max_texture_dimension: u32) -> Self {
        Self {
            max_texture_dimension,
            next_node_handle: 1,
            next_texture_handle: 1,
            ..Self::default()
        }
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
                let content = self.resolve_content(session, content)?;
                let node = scene.create_root_in_layer(properties.into_scene()?, layer);
                scene
                    .set_content(node, content)
                    .expect("a newly created scene root is valid");
                let handle = self.allocate_node(session, node)?;
                Ok(SceneCommandResult::Node(handle))
            }
            SceneCommand::CreateChild {
                parent,
                properties,
                content,
            } => {
                let parent = self.node_for(session, parent)?;
                let content = self.resolve_content(session, content)?;
                let node = scene
                    .create_child(parent, properties.into_scene()?)
                    .map_err(ProgramSceneError::Scene)?;
                scene
                    .set_content(node, content)
                    .expect("a newly created scene child is valid");
                let handle = self.allocate_node(session, node)?;
                Ok(SceneCommandResult::Node(handle))
            }
            SceneCommand::UpdateNode { node, properties } => {
                let node = self.node_for(session, node)?;
                let current = scene
                    .node(node)
                    .ok_or(ProgramSceneError::InvalidNodeHandle(node_handle_for(
                        &self.nodes,
                        node,
                    )?))?
                    .properties();
                scene
                    .set_properties(node, properties.apply(current)?)
                    .map_err(ProgramSceneError::Scene)?;
                Ok(SceneCommandResult::Acknowledged)
            }
            SceneCommand::SetContent { node, content } => {
                let node = self.node_for(session, node)?;
                self.set_content(session, node, content, scene)?;
                Ok(SceneCommandResult::Acknowledged)
            }
            SceneCommand::Reparent { node, parent } => {
                let node = self.node_for(session, node)?;
                let parent = parent
                    .map(|handle| self.node_for(session, handle))
                    .transpose()?;
                scene
                    .reparent(node, parent)
                    .map_err(ProgramSceneError::Scene)?;
                Ok(SceneCommandResult::Acknowledged)
            }
            SceneCommand::RemoveNode { node } => {
                let node = self.node_for(session, node)?;
                scene
                    .remove_subtree(node)
                    .map_err(ProgramSceneError::Scene)?;
                self.nodes
                    .retain(|_, owned| scene.node(owned.node).is_some());
                Ok(SceneCommandResult::Acknowledged)
            }
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
        }
    }

    pub fn cleanup_session(
        &mut self,
        session: SessionId,
        scene: &mut Scene,
        textures: &mut TextureStore,
    ) {
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
        self.session_texture_bytes.remove(&session);
    }

    fn set_content(
        &self,
        session: SessionId,
        node: NodeId,
        content: ContentInput,
        scene: &mut Scene,
    ) -> Result<(), ProgramSceneError> {
        let content = self.resolve_content(session, content)?;
        scene
            .set_content(node, content)
            .map_err(ProgramSceneError::Scene)
    }

    fn resolve_content(
        &self,
        session: SessionId,
        content: ContentInput,
    ) -> Result<NodeContent, ProgramSceneError> {
        Ok(match content {
            ContentInput::Empty => NodeContent::Empty,
            ContentInput::Solid { color } => NodeContent::Solid(SolidNode::new(color)),
            ContentInput::Image { texture, opacity } => NodeContent::Image(
                ImageNode::new(self.texture_for(session, texture)?, opacity)
                    .map_err(ProgramSceneError::Image)?,
            ),
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
    use super::*;
    use crate::protocol::SessionManager;

    fn session() -> SessionId {
        let mut manager = SessionManager::default();
        manager
            .accept(
                crate::protocol::ClientHello {
                    protocol_version: crate::protocol::MLP_PROTOCOL_VERSION,
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
            protocol_version: crate::protocol::MLP_PROTOCOL_VERSION,
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
        }
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
        resources.cleanup_session(session, &mut scene, &mut textures);
        assert!(scene.is_empty());
        assert!(textures.is_empty());
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
                    },
                },
                &mut scene,
                &mut textures,
            ),
            Err(ProgramSceneError::InvalidTextureHandle(_))
        ));
        assert!(scene.is_empty());
    }
}

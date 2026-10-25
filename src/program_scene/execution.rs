//! Ordered retained-scene command execution and presentation commits.
//!
//! This module owns the command-facing mutation path. Ownership lookups,
//! quota accounting, overlap reconciliation, and resource cleanup stay in
//! the sibling `resources` module so every command uses one retained state.

use super::*;

impl ProgramSceneResources {
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

    pub(super) fn create_clp_node(
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

    pub(super) fn set_clp_node(
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
}

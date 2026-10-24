// Private retained-scene tests retain direct access to implementation details.
// Keep this suite separate from the scene ownership and command code.

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

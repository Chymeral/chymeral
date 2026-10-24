// Private transport tests retain direct access to the module under test.
// Keeping them separate makes the lifecycle and wire code easier to navigate.

    use std::time::{Duration, Instant};
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::mpsc,
    };

    use interprocess::{
        TryClone as _,
        local_socket::{GenericNamespaced, ToNsName, prelude::LocalSocketStream},
    };
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    use crate::runtime::KeyEvent;
    use crate::{
        program_scene::SceneBatchResult,
        protocol::{ClientHello, ProgramInitContext, SessionManager},
    };

    use super::*;

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FixtureCorpus {
        format: String,
        fixture_version: u32,
        protocol: FixtureProtocolVersion,
        transport: FixtureTransport,
        vectors: Vec<FixtureVector>,
    }

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FixtureProtocolVersion {
        major: u16,
        minor: u16,
    }

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FixtureTransport {
        length_prefix: String,
        json_discriminator: String,
        binary_discriminator: String,
        maximum_frame_bytes: usize,
    }

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FixtureVector {
        id: String,
        description: String,
        frames: Vec<FixtureFrame>,
    }

    #[derive(Debug, serde::Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum FixtureDirection {
        ClientToHost,
        HostToClient,
    }

    #[derive(Debug, serde::Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum FixtureFrameKind {
        Json,
        Binary,
    }

    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FixtureFrame {
        direction: FixtureDirection,
        kind: FixtureFrameKind,
        #[serde(default)]
        message: Option<Value>,
        #[serde(default)]
        bytes_hex: Option<String>,
    }

    #[test]
    fn validates_the_canonical_clp_v2_fixture_corpus() {
        let fixture_bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/protocol/fixtures/v2.0/fixtures.json"
        ));
        let expected_hash = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/protocol/fixtures/v2.0/SHA256SUMS"
        ))
            .split_whitespace()
            .next()
            .expect("fixture SHA256SUMS must contain a checksum");
        assert_eq!(
            format!("{:x}", Sha256::digest(fixture_bytes)),
            expected_hash
        );

        let corpus: FixtureCorpus =
            serde_json::from_str(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/protocol/fixtures/v2.0/fixtures.json"
            )))
                .expect("the canonical CLP fixture corpus must be valid JSON");

        assert_eq!(corpus.format, "chymeral-clp-fixture-corpus");
        assert_eq!(corpus.fixture_version, 1);
        assert_eq!((corpus.protocol.major, corpus.protocol.minor), (2, 0));
        assert_eq!(
            corpus.transport.length_prefix,
            "unsigned 32-bit big-endian payload length including the one-byte discriminator"
        );
        assert_eq!(corpus.transport.json_discriminator, "J");
        assert_eq!(corpus.transport.binary_discriminator, "B");
        assert_eq!(corpus.transport.maximum_frame_bytes, MAX_FRAME_BYTES);

        let expected_vectors: BTreeSet<_> = [
            "delivery-errors-and-fault-close",
            "image-upload-and-cleanup",
            "lifecycle-and-subscriptions",
            "scene-text-and-overlap",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let mut vector_ids = BTreeSet::new();
        let mut client_message_types = BTreeSet::new();
        let mut host_message_types = BTreeSet::new();

        for vector in corpus.vectors {
            assert!(!vector.id.is_empty());
            assert!(!vector.description.is_empty());
            assert!(vector_ids.insert(vector.id));

            let mut pending_upload_length = None;
            let mut pending_request_ids = BTreeSet::new();
            for frame in vector.frames {
                match (frame.direction, frame.kind) {
                    (FixtureDirection::ClientToHost, FixtureFrameKind::Json) => {
                        assert!(pending_upload_length.is_none());
                        assert!(frame.bytes_hex.is_none());
                        let message = frame.message.expect("JSON frames need a message");
                        let message_type = fixture_message_type(&message);
                        client_message_types.insert(message_type.to_owned());
                        let message: WireClientMessage = serde_json::from_value(message)
                            .expect("client fixture messages must match the CLP wire type");
                        if let WireClientMessage::Command {
                            request_id,
                            command,
                        } = &message
                        {
                            assert!(
                                pending_request_ids.insert(request_id.clone()),
                                "fixture command request IDs must be unique within a vector"
                            );
                            pending_upload_length = command
                                .upload_length()
                                .expect("fixture image uploads must have valid dimensions");
                        }
                    }
                    (FixtureDirection::ClientToHost, FixtureFrameKind::Binary) => {
                        assert!(frame.message.is_none());
                        let bytes = fixture_hex_bytes(
                            &frame.bytes_hex.expect("binary frames need fixture bytes"),
                        );
                        assert_eq!(
                            pending_upload_length.take(),
                            Some(bytes.len()),
                            "a binary fixture frame must immediately complete its image upload"
                        );
                        let frame = raw_frame_bytes(b'B', &bytes)
                            .expect("fixture binary frames must fit the CLP transport");
                        assert_eq!(
                            usize::try_from(u32::from_be_bytes(frame[..4].try_into().unwrap()))
                                .unwrap(),
                            bytes.len() + 1
                        );
                        assert_eq!(frame[4], b'B');
                    }
                    (FixtureDirection::HostToClient, FixtureFrameKind::Json) => {
                        assert!(pending_upload_length.is_none());
                        assert!(frame.bytes_hex.is_none());
                        let message = frame.message.expect("JSON frames need a message");
                        let message_type = fixture_host_message_type(&message);
                        host_message_types.insert(message_type.to_owned());
                        if matches!(message_type, "result" | "command_error") {
                            let request_id = message
                                .as_object()
                                .and_then(|message| message.get("request_id"))
                                .and_then(Value::as_str)
                                .expect("command responses need a string request_id");
                            assert!(
                                pending_request_ids.remove(request_id),
                                "fixture command response must follow its request in the same vector"
                            );
                        }
                    }
                    (FixtureDirection::HostToClient, FixtureFrameKind::Binary) => {
                        panic!("CLP v2.0 does not send binary host frames")
                    }
                }
            }
            assert!(pending_upload_length.is_none());
            assert!(
                pending_request_ids.is_empty(),
                "fixture vectors must not leave commands without a response"
            );
        }

        assert_eq!(vector_ids, expected_vectors);
        assert_eq!(
            client_message_types,
            [
                "close",
                "command",
                "fault",
                "hello",
                "init_complete",
                "started",
                "subscribe",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect()
        );
        assert_eq!(
            host_message_types,
            [
                "closed",
                "command_error",
                "delivery_gap",
                "event",
                "frame",
                "init",
                "resize",
                "result",
                "start",
                "subscribed",
                "tick",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect()
        );
    }

    #[test]
    fn validates_the_canonical_clp_v2_1_presentation_fixture_corpus() {
        let fixture_bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/protocol/fixtures/v2.1/fixtures.json"
        ));
        let expected_hash = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/protocol/fixtures/v2.1/SHA256SUMS"
        ))
            .split_whitespace()
            .next()
            .expect("fixture SHA256SUMS must contain a checksum");
        assert_eq!(
            format!("{:x}", Sha256::digest(fixture_bytes)),
            expected_hash
        );

        let corpus: FixtureCorpus =
            serde_json::from_str(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/protocol/fixtures/v2.1/fixtures.json"
            )))
                .expect("the CLP v2.1 fixture corpus must be valid JSON");
        assert_eq!(corpus.format, "chymeral-clp-fixture-corpus");
        assert_eq!(corpus.fixture_version, 1);
        assert_eq!((corpus.protocol.major, corpus.protocol.minor), (2, 1));
        assert_eq!(
            corpus.transport.length_prefix,
            "unsigned 32-bit big-endian payload length including the one-byte discriminator"
        );
        assert_eq!(corpus.transport.json_discriminator, "J");
        assert_eq!(corpus.transport.binary_discriminator, "B");
        assert_eq!(corpus.transport.maximum_frame_bytes, MAX_FRAME_BYTES);

        let mut vector_ids = BTreeSet::new();
        let mut client_message_types = BTreeSet::new();
        let mut host_message_types = BTreeSet::new();
        for vector in corpus.vectors {
            assert!(!vector.id.is_empty());
            assert!(!vector.description.is_empty());
            assert!(vector_ids.insert(vector.id));

            let mut pending_upload_length = None;
            for frame in vector.frames {
                match (frame.direction, frame.kind) {
                    (FixtureDirection::ClientToHost, FixtureFrameKind::Json) => {
                        assert!(pending_upload_length.is_none());
                        assert!(frame.bytes_hex.is_none());
                        let message = frame.message.expect("JSON frames need a message");
                        let message_type = fixture_message_type(&message);
                        client_message_types.insert(message_type.to_owned());
                        let message: WireClientMessage = serde_json::from_value(message)
                            .expect("client fixture messages must match the CLP wire type");
                        if let WireClientMessage::Command { command, .. } = message {
                            pending_upload_length = command
                                .upload_length()
                                .expect("fixture image uploads must have valid dimensions");
                        }
                    }
                    (FixtureDirection::ClientToHost, FixtureFrameKind::Binary) => {
                        assert!(frame.message.is_none());
                        let bytes = fixture_hex_bytes(
                            &frame.bytes_hex.expect("binary frames need fixture bytes"),
                        );
                        assert_eq!(
                            pending_upload_length.take(),
                            Some(bytes.len()),
                            "a binary fixture frame must immediately complete its image upload"
                        );
                    }
                    (FixtureDirection::HostToClient, FixtureFrameKind::Json) => {
                        assert!(pending_upload_length.is_none());
                        assert!(frame.bytes_hex.is_none());
                        let message = frame.message.expect("JSON frames need a message");
                        let message_type = fixture_host_message_type(&message);
                        host_message_types.insert(message_type.to_owned());
                    }
                    (FixtureDirection::HostToClient, FixtureFrameKind::Binary) => {
                        panic!("CLP v2.1 does not send binary host frames")
                    }
                }
            }
            assert!(pending_upload_length.is_none());
        }

        assert_eq!(
            vector_ids,
            ["application-window-metadata", "frame-presentation"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
        assert_eq!(
            client_message_types,
            [
                "close",
                "command",
                "hello",
                "init_complete",
                "present_scene",
                "started"
            ]
            .into_iter()
            .map(str::to_owned)
            .collect()
        );
        assert_eq!(
            host_message_types,
            ["closed", "init", "presentation_status", "result", "start"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
    }

    #[test]
    fn validates_the_canonical_clp_v2_4_scene_batch_fixture_corpus() {
        let fixture_bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/protocol/fixtures/v2.4/fixtures.json"
        ));
        let expected_hash = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/protocol/fixtures/v2.4/SHA256SUMS"
        ))
            .split_whitespace()
            .next()
            .expect("fixture SHA256SUMS must contain a checksum");
        assert_eq!(
            format!("{:x}", Sha256::digest(fixture_bytes)),
            expected_hash
        );

        let corpus: FixtureCorpus =
            serde_json::from_str(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/protocol/fixtures/v2.4/fixtures.json"
            )))
                .expect("the CLP v2.4 fixture corpus must be valid JSON");
        assert_eq!(corpus.format, "chymeral-clp-fixture-corpus");
        assert_eq!(corpus.fixture_version, 1);
        assert_eq!((corpus.protocol.major, corpus.protocol.minor), (2, 4));
        assert_eq!(
            corpus.transport.length_prefix,
            "unsigned 32-bit big-endian payload length including the one-byte discriminator"
        );
        assert_eq!(corpus.transport.json_discriminator, "J");
        assert_eq!(corpus.transport.binary_discriminator, "B");
        assert_eq!(corpus.transport.maximum_frame_bytes, MAX_FRAME_BYTES);

        let mut vector_ids = BTreeSet::new();
        let mut client_message_types = BTreeSet::new();
        let mut host_message_types = BTreeSet::new();
        let mut scene_batch_count = 0;
        for vector in corpus.vectors {
            assert!(!vector.id.is_empty());
            assert!(!vector.description.is_empty());
            assert!(vector_ids.insert(vector.id));

            let mut pending_request_ids = BTreeSet::new();
            for frame in vector.frames {
                match (frame.direction, frame.kind) {
                    (FixtureDirection::ClientToHost, FixtureFrameKind::Json) => {
                        assert!(frame.bytes_hex.is_none());
                        let message = frame.message.expect("JSON frames need a message");
                        let message_type = fixture_message_type(&message);
                        client_message_types.insert(message_type.to_owned());
                        let message: WireClientMessage = serde_json::from_value(message)
                            .expect("client fixture messages must match the CLP wire type");
                        if let WireClientMessage::Command {
                            request_id,
                            command,
                        } = message
                        {
                            assert!(
                                pending_request_ids.insert(request_id),
                                "fixture command request IDs must be unique within a vector"
                            );
                            assert_eq!(command.upload_length().unwrap(), None);
                            if let WireSceneCommand::SceneBatch { operations } = command.as_ref() {
                                scene_batch_count += 1;
                                assert_eq!(operations.len(), 5);
                            }
                        }
                    }
                    (FixtureDirection::HostToClient, FixtureFrameKind::Json) => {
                        assert!(frame.bytes_hex.is_none());
                        let message = frame.message.expect("JSON frames need a message");
                        let message_type = fixture_host_message_type(&message);
                        host_message_types.insert(message_type.to_owned());
                        if matches!(message_type, "result" | "command_error") {
                            let request_id = message
                                .as_object()
                                .and_then(|message| message.get("request_id"))
                                .and_then(Value::as_str)
                                .expect("command responses need a string request_id");
                            assert!(
                                pending_request_ids.remove(request_id),
                                "fixture command response must follow its request in the same vector"
                            );
                        }
                        if message_type == "result" {
                            assert_eq!(message["result"]["kind"], "scene_batch");
                            assert!(message["result"]["nodes"].get("temporary").is_none());
                            assert_eq!(
                                message["result"]["operations"].as_array().unwrap().len(),
                                5
                            );
                        }
                    }
                    (FixtureDirection::ClientToHost, FixtureFrameKind::Binary)
                    | (FixtureDirection::HostToClient, FixtureFrameKind::Binary) => {
                        panic!("CLP v2.4 scene-batch fixtures do not use binary frames")
                    }
                }
            }
            assert!(
                pending_request_ids.is_empty(),
                "fixture vectors must not leave commands without a response"
            );
        }

        assert_eq!(
            vector_ids,
            ["atomic-scene-batch"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
        assert_eq!(scene_batch_count, 1);
        assert_eq!(
            client_message_types,
            ["close", "command", "hello", "init_complete", "started"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
        assert_eq!(
            host_message_types,
            ["closed", "init", "result", "start"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
    }

    fn fixture_message_type(message: &Value) -> &str {
        message
            .as_object()
            .and_then(|message| message.get("type"))
            .and_then(Value::as_str)
            .expect("fixture messages must have a string type")
    }

    fn fixture_host_message_type(message: &Value) -> &str {
        let object = message
            .as_object()
            .expect("host fixture messages must be JSON objects");
        let message_type = fixture_message_type(message);
        let required_fields: &[&str] = match message_type {
            "init" => &["protocol", "session_id", "capabilities", "context"],
            "start" => &["session_id"],
            "subscribed" => &["session_id", "subscriptions"],
            "resize" => &[
                "session_id",
                "physical_width",
                "physical_height",
                "logical_width",
                "logical_height",
                "scale_factor",
            ],
            "tick" | "frame" => &["session_id", "dt_ns"],
            "event" => &["session_id", "event"],
            "delivery_gap" => &["session_id", "categories"],
            "presentation_status" => &["session_id", "presentation_id", "status"],
            "result" => &["request_id", "result"],
            "command_error" => &["request_id", "code", "message"],
            "closed" => &["session_id", "reason"],
            _ => panic!("unsupported host fixture message type {message_type}"),
        };
        for field in required_fields {
            assert!(
                object.contains_key(*field),
                "{message_type} fixture messages need {field}"
            );
        }
        message_type
    }

    fn fixture_hex_bytes(value: &str) -> Vec<u8> {
        assert!(value.len().is_multiple_of(2));
        (0..value.len())
            .step_by(2)
            .map(|offset| u8::from_str_radix(&value[offset..offset + 2], 16).unwrap())
            .collect()
    }

    fn connect(server: &ClpServer) -> LocalSocketStream {
        try_connect(server).unwrap()
    }

    fn try_connect(server: &ClpServer) -> io::Result<LocalSocketStream> {
        let name = server
            .environment()
            .endpoint()
            .to_ns_name::<GenericNamespaced>()
            .unwrap();
        LocalSocketStream::connect(name)
    }

    fn hello(server: &ClpServer, capabilities: Vec<&str>) -> WireClientMessage {
        WireClientMessage::Hello {
            protocol: WireProtocolVersion {
                major: crate::protocol::CLP_PROTOCOL_VERSION.major(),
                minor: crate::protocol::CLP_PROTOCOL_VERSION.minor(),
            },
            token: server.environment().authentication_token().to_owned(),
            capabilities: capabilities
                .into_iter()
                .map(|capability| match capability {
                    "scene" => WireCapability::Scene,
                    "images" => WireCapability::Images,
                    "input" => WireCapability::Input,
                    "timing" => WireCapability::Timing,
                    "overlap" => WireCapability::Overlap,
                    "external_drop" => WireCapability::ExternalDrop,
                    "text" => WireCapability::Text,
                    "presentation" => WireCapability::Presentation,
                    "window_metadata" => WireCapability::WindowMetadata,
                    "scene_batch" => WireCapability::SceneBatch,
                    _ => unreachable!(),
                })
                .collect(),
        }
    }

    fn send_client(stream: &mut LocalSocketStream, message: WireClientMessage) {
        write_frame(stream, &message).unwrap();
    }

    struct IntermittentWriter {
        bytes: Vec<u8>,
        write_attempts: usize,
    }

    struct BoundedWriter {
        bytes: Vec<u8>,
        maximum_write_size: usize,
        largest_write: usize,
    }

    impl Write for BoundedWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.largest_write = self.largest_write.max(bytes.len());
            if bytes.len() > self.maximum_write_size {
                return Ok(0);
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Write for IntermittentWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.write_attempts += 1;
            if self.write_attempts == 2 {
                return Ok(0);
            }
            if self.write_attempts == 3 {
                return Err(io::Error::from(io::ErrorKind::WouldBlock));
            }
            let count = bytes.len().min(2);
            self.bytes.extend_from_slice(&bytes[..count]);
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn completes_a_frame_after_zero_byte_and_transient_output_backpressure() {
        let mut writer = IntermittentWriter {
            bytes: Vec::new(),
            write_attempts: 0,
        };

        write_raw_frame(&mut writer, b'J', b"payload").unwrap();

        assert_eq!(
            writer.bytes,
            [0, 0, 0, 8, b'J', b'p', b'a', b'y', b'l', b'o', b'a', b'd']
        );
    }

    #[test]
    fn retains_queued_output_until_transient_backpressure_clears() {
        let first = json!({ "type": "event", "sequence": 1 });
        let second = json!({ "type": "event", "sequence": 2 });
        let expected = [
            json_frame_bytes(&first).unwrap(),
            json_frame_bytes(&second).unwrap(),
        ]
        .concat();
        let mut frames = RunningFrameWriter::default();
        frames.queue(&first).unwrap();
        frames.queue(&second).unwrap();
        let mut writer = IntermittentWriter {
            bytes: Vec::new(),
            write_attempts: 0,
        };

        while !frames.is_empty() {
            frames.flush(&mut writer).unwrap();
        }

        assert_eq!(writer.bytes, expected);
    }

    #[test]
    fn fragments_large_queued_output_for_bounded_transports() {
        let message = json!({
            "type": "result",
            "payload": "x".repeat(MAX_RUNNING_FRAME_WRITE_CHUNK_BYTES * 3),
        });
        let expected = json_frame_bytes(&message).unwrap();
        let mut frames = RunningFrameWriter::default();
        frames.queue(&message).unwrap();
        let mut writer = BoundedWriter {
            bytes: Vec::new(),
            maximum_write_size: MAX_RUNNING_FRAME_WRITE_CHUNK_BYTES,
            largest_write: 0,
        };

        frames.flush(&mut writer).unwrap();

        assert!(frames.is_empty());
        assert_eq!(writer.bytes, expected);
        assert_eq!(writer.largest_write, MAX_RUNNING_FRAME_WRITE_CHUNK_BYTES);
    }

    #[test]
    fn presentation_mailbox_retains_only_the_latest_packet_per_session() {
        let mailbox = PresentationMailbox::default();
        let mut sessions = SessionManager::new(CapabilitySet::ALL);
        let first_session = sessions
            .accept(
                ClientHello {
                    protocol_version: crate::protocol::CLP_PROTOCOL_VERSION,
                    requested_capabilities: CapabilitySet::ALL,
                },
                ProgramInitContext::default(),
            )
            .unwrap()
            .0;
        let second_session = sessions
            .accept(
                ClientHello {
                    protocol_version: crate::protocol::CLP_PROTOCOL_VERSION,
                    requested_capabilities: CapabilitySet::ALL,
                },
                ProgramInitContext::default(),
            )
            .unwrap()
            .0;
        let packet = |presentation_id: &str| PresentationPacket {
            presentation_id: presentation_id.to_owned(),
            updates: vec![PresentationUpdate {
                node: NodeHandle::new(1),
                properties: PresentationPropertiesPatch {
                    x: Some(1.0),
                    ..PresentationPropertiesPatch::default()
                },
            }],
        };

        assert!(mailbox.submit(first_session, packet("first")).is_none());
        let superseded = mailbox
            .submit(first_session, packet("latest"))
            .expect("the earlier packet is replaced");
        assert_eq!(superseded.packet.presentation_id, "first");
        assert!(mailbox.submit(second_session, packet("other")).is_none());

        let packets = mailbox.take_all();
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0].session_id, first_session);
        assert_eq!(packets[0].packet.presentation_id, "latest");
        assert_eq!(packets[1].session_id, second_session);
        assert_eq!(packets[1].packet.presentation_id, "other");
    }

    #[test]
    fn accepts_frame_presentation_without_a_command_round_trip() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["scene", "presentation"]));
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);
        let session_id = wait_for_session_started(&server);
        server.activate_session(session_id, 900, 600, 1.0);

        send_client(
            &mut client,
            WireClientMessage::PresentScene {
                presentation_id: "packet-1".to_owned(),
                updates: vec![WirePresentationUpdate {
                    node: "node-1".to_owned(),
                    properties: WirePresentationPropertiesPatch {
                        x: Some(48.0),
                        y: Some(24.0),
                        ..WirePresentationPropertiesPatch::default()
                    },
                }],
            },
        );

        let deadline = Instant::now() + Duration::from_secs(1);
        let submission = loop {
            let submissions = server.drain_presentation_packets();
            if let Some(submission) = submissions.into_iter().next() {
                break submission;
            }
            assert!(
                Instant::now() < deadline,
                "the presentation packet did not arrive"
            );
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(submission.session_id(), session_id);
        assert_eq!(submission.packet().presentation_id, "packet-1");
        assert_eq!(submission.packet().updates.len(), 1);
        assert!(
            !server
                .drain_host_actions()
                .into_iter()
                .any(|action| matches!(action, HostAction::Command(_)))
        );

        server.report_presentation_presented(session_id, "packet-1".to_owned());
        let status = receive_host(&mut client);
        assert_eq!(status["type"], "presentation_status");
        assert_eq!(status["session_id"], session_id.to_string());
        assert_eq!(status["presentation_id"], "packet-1");
        assert_eq!(status["status"], "presented");

        send_client(&mut client, WireClientMessage::Close);
        let _ = receive_host(&mut client);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn preserves_a_running_session_during_named_pipe_backpressure() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["input"]));
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);
        let session_id = wait_for_session_started(&server);
        server.activate_session(session_id, 900, 600, 1.0);
        let resize = receive_host(&mut client);
        assert_eq!(resize["type"], "resize");
        send_client(
            &mut client,
            WireClientMessage::Subscribe {
                subscriptions: vec![WireSubscription::RawInput],
            },
        );
        let _ = receive_host(&mut client);

        let payload = vec![0_u8; 2 * 1024];
        server.broadcast_event(&EngineEvent::Custom(crate::runtime::CustomEvent {
            name: "backpressure".to_owned(),
            payload: payload.clone(),
        }));
        thread::sleep(WRITE_STALL_TIMEOUT + Duration::from_millis(250));

        assert_eq!(server.session_count().unwrap(), 1);
        let event = receive_host(&mut client);
        assert_eq!(event["type"], "event");
        assert_eq!(event["event"]["kind"], "custom");
        assert_eq!(event["event"]["name"], "backpressure");
        assert_eq!(
            event["event"]["payload"].as_array().unwrap().len(),
            payload.len()
        );

        send_client(&mut client, WireClientMessage::Close);
        let closed = receive_host(&mut client);
        assert_eq!(closed["type"], "closed");
        assert_eq!(closed["reason"], json!({ "kind": "client_requested" }));
        wait_for_session_count(&server, 0);
    }

    fn receive_host(stream: &mut LocalSocketStream) -> Value {
        receive_host_with_reader(stream, &mut FrameReader::default())
    }

    fn receive_host_async(
        stream: &LocalSocketStream,
    ) -> (
        mpsc::Receiver<Result<Value, String>>,
        thread::JoinHandle<()>,
    ) {
        let mut reader = stream.try_clone().unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        let join_handle = thread::spawn(move || {
            let message = match FrameReader::default().read_next(&mut reader, None, None) {
                Ok(FrameRead::Message(message)) => Ok(message),
                Ok(FrameRead::Disconnected) => {
                    Err("the CLP server disconnected before replying".to_owned())
                }
                Ok(FrameRead::Shutdown) => {
                    Err("a standalone CLP client cannot receive host shutdown".to_owned())
                }
                Err(error) => Err(format!("could not read the CLP host response: {error:#}")),
            };
            let _ = sender.send(message);
        });
        (receiver, join_handle)
    }

    fn receive_host_with_reader(
        stream: &mut LocalSocketStream,
        frame_reader: &mut FrameReader,
    ) -> Value {
        match frame_reader.read_next(stream, None, None).unwrap() {
            FrameRead::Message(message) => message,
            FrameRead::Disconnected => panic!("the CLP server disconnected before replying"),
            FrameRead::Shutdown => panic!("a standalone CLP client cannot receive host shutdown"),
        }
    }

    fn wait_for_session_count(server: &ClpServer, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if server.session_count().unwrap() == expected {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(server.session_count().unwrap(), expected);
    }

    fn wait_for_host_command(server: &ClpServer) -> HostCommandRequest {
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if let Some(request) =
                server
                    .drain_host_actions()
                    .into_iter()
                    .find_map(|action| match action {
                        HostAction::Command(request) => Some(*request),
                        HostAction::Cleanup { .. } | HostAction::SessionStarted { .. } => None,
                    })
            {
                return request;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("the CLP host action did not arrive")
    }

    fn wait_for_session_started(server: &ClpServer) -> SessionId {
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if let Some(session_id) =
                server
                    .drain_host_actions()
                    .into_iter()
                    .find_map(|action| match action {
                        HostAction::SessionStarted { session_id } => Some(session_id),
                        HostAction::Command(_) | HostAction::Cleanup { .. } => None,
                    })
            {
                return session_id;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("the CLP session did not enter its running state")
    }

    fn session_id() -> SessionId {
        let mut manager = SessionManager::new(CapabilitySet::ALL);
        manager
            .accept(
                ClientHello {
                    protocol_version: crate::protocol::CLP_PROTOCOL_VERSION,
                    requested_capabilities: CapabilitySet::ALL,
                },
                ProgramInitContext::default(),
            )
            .unwrap()
            .0
    }

    fn next_notification(queue: &NotificationQueue) -> HostNotification {
        match queue.try_pop() {
            NotificationPoll::Notification(notification) => notification,
            NotificationPoll::Overloaded => panic!("the notification queue is overloaded"),
            NotificationPoll::Empty => panic!("the notification queue is empty"),
        }
    }

    #[test]
    fn activation_delivers_resize_first_and_gates_opt_in_notifications() {
        let notifications = SessionNotifications::default();
        let session_id = session_id();
        let queue = notifications.register(
            session_id,
            CapabilitySet::from_capabilities(&[Capability::Input, Capability::Timing]),
        );

        notifications.broadcast_matching(
            HostNotification::Tick {
                dt_ns: "1".to_owned(),
            },
            |_| true,
        );
        assert!(matches!(queue.try_pop(), NotificationPoll::Empty));

        assert!(notifications.activate(session_id, resize_notification(900, 600, 1.5)));
        assert!(matches!(
            next_notification(&queue),
            HostNotification::Resize {
                physical_width: 900,
                physical_height: 600,
                ..
            }
        ));

        notifications.broadcast_matching(
            HostNotification::Tick {
                dt_ns: "2".to_owned(),
            },
            |session| session.subscriptions.contains(&Subscription::Timing),
        );
        assert!(matches!(queue.try_pop(), NotificationPoll::Empty));

        notifications
            .set_subscriptions(session_id, BTreeSet::from([Subscription::Timing]))
            .unwrap();
        notifications.broadcast_matching(
            HostNotification::Tick {
                dt_ns: "3".to_owned(),
            },
            |session| session.subscriptions.contains(&Subscription::Timing),
        );
        assert!(matches!(
            next_notification(&queue),
            HostNotification::Tick { dt_ns } if dt_ns == "3"
        ));
    }

    #[test]
    fn coalesces_continuous_notifications_to_the_latest_sample() {
        let notifications = SessionNotifications::default();
        let session_id = session_id();
        let queue = notifications.register(session_id, CapabilitySet::NONE);
        assert!(notifications.activate(session_id, resize_notification(1, 1, 1.0)));

        notifications.broadcast_matching(
            HostNotification::Tick {
                dt_ns: "first".to_owned(),
            },
            |_| true,
        );
        notifications.broadcast_matching(
            HostNotification::Event(WireEvent::Focus {
                state: WireFocusState::Gained,
            }),
            |_| true,
        );
        notifications.broadcast_matching(
            HostNotification::Tick {
                dt_ns: "latest".to_owned(),
            },
            |_| true,
        );

        assert!(matches!(
            next_notification(&queue),
            HostNotification::DeliveryGap { categories }
                if categories == BTreeSet::from([DeliveryCategory::Timing])
        ));
        assert!(matches!(
            next_notification(&queue),
            HostNotification::Event(WireEvent::Focus { .. })
        ));
        assert!(matches!(
            next_notification(&queue),
            HostNotification::Tick { dt_ns } if dt_ns == "latest"
        ));
        assert_eq!(
            notifications.metrics(),
            ClpNotificationMetrics {
                coalesced_notifications: 1,
                delivery_gaps_emitted: 1,
                ..ClpNotificationMetrics::default()
            }
        );
    }

    #[test]
    fn coalesces_presentation_statuses_without_a_delivery_gap() {
        let notifications = SessionNotifications::default();
        let session_id = session_id();
        let queue = notifications.register(
            session_id,
            CapabilitySet::from_capabilities(&[Capability::Presentation]),
        );
        assert!(notifications.activate(session_id, resize_notification(1, 1, 1.0)));

        enqueue_presentation_status(
            &notifications,
            session_id,
            "first".to_owned(),
            PresentationStatus::Presented,
        );
        enqueue_presentation_status(
            &notifications,
            session_id,
            "latest".to_owned(),
            PresentationStatus::Rejected {
                code: "invalid_node_change".to_owned(),
                message: "a text node has intrinsic bounds".to_owned(),
            },
        );

        assert!(matches!(
            next_notification(&queue),
            HostNotification::PresentationStatus {
                presentation_id,
                status: PresentationStatus::Rejected { code, message },
            } if presentation_id == "latest"
                && code == "invalid_node_change"
                && message == "a text node has intrinsic bounds"
        ));
        assert!(matches!(queue.try_pop(), NotificationPoll::Empty));
        assert_eq!(
            notifications.metrics(),
            ClpNotificationMetrics {
                coalesced_notifications: 1,
                ..ClpNotificationMetrics::default()
            }
        );
    }

    #[test]
    fn coalesces_terminal_output_availability() {
        let notifications = SessionNotifications::default();
        let session_id = session_id();
        let queue = notifications.register(session_id, CapabilitySet::NONE);
        assert!(notifications.activate(session_id, resize_notification(1, 1, 1.0)));

        for _ in 0..2 {
            notifications.broadcast_matching(
                HostNotification::Event(WireEvent::Terminal {
                    state: WireTerminalState::OutputAvailable,
                }),
                |_| true,
            );
        }

        assert!(matches!(
            next_notification(&queue),
            HostNotification::DeliveryGap { categories }
                if categories == BTreeSet::from([DeliveryCategory::Terminal])
        ));
        assert!(matches!(
            next_notification(&queue),
            HostNotification::Event(WireEvent::Terminal {
                state: WireTerminalState::OutputAvailable,
            })
        ));
    }

    #[test]
    fn prioritizes_a_discrete_notification_over_a_pending_continuous_one() {
        let notifications = SessionNotifications::default();
        let session_id = session_id();
        let queue = notifications.register(session_id, CapabilitySet::NONE);
        assert!(notifications.activate(session_id, resize_notification(1, 1, 1.0)));

        for _ in 0..MAX_PENDING_HOST_NOTIFICATIONS - 1 {
            notifications.broadcast_matching(
                HostNotification::Event(WireEvent::Focus {
                    state: WireFocusState::Gained,
                }),
                |_| true,
            );
        }
        notifications.broadcast_matching(
            HostNotification::Frame {
                dt_ns: "discarded".to_owned(),
            },
            |_| true,
        );
        notifications.broadcast_matching(
            HostNotification::Event(WireEvent::Key {
                key: WireKey::Character {
                    value: "a".to_owned(),
                },
                text: Some("a".to_owned()),
                state: WireKeyState::Pressed,
                modifiers: WireModifiers {
                    shift: false,
                    control: false,
                    alt: false,
                    super_key: false,
                },
                repeat: false,
            }),
            |_| true,
        );

        assert!(matches!(
            next_notification(&queue),
            HostNotification::DeliveryGap { categories }
                if categories == BTreeSet::from([DeliveryCategory::Timing])
        ));
        for _ in 0..MAX_PENDING_HOST_NOTIFICATIONS - 1 {
            assert!(matches!(
                next_notification(&queue),
                HostNotification::Event(WireEvent::Focus { .. })
            ));
        }
        assert!(matches!(
            next_notification(&queue),
            HostNotification::Event(WireEvent::Key { .. })
        ));
        assert_eq!(
            notifications.metrics(),
            ClpNotificationMetrics {
                dropped_continuous_notifications: 1,
                delivery_gaps_emitted: 1,
                ..ClpNotificationMetrics::default()
            }
        );
    }

    #[test]
    fn overloads_a_session_instead_of_dropping_a_discrete_notification() {
        let notifications = SessionNotifications::default();
        let session_id = session_id();
        let queue = notifications.register(session_id, CapabilitySet::NONE);
        assert!(notifications.activate(session_id, resize_notification(1, 1, 1.0)));

        for _ in 0..MAX_PENDING_HOST_NOTIFICATIONS {
            notifications.broadcast_matching(
                HostNotification::Event(WireEvent::Focus {
                    state: WireFocusState::Gained,
                }),
                |_| true,
            );
        }
        notifications.broadcast_matching(
            HostNotification::Event(WireEvent::Key {
                key: WireKey::Character {
                    value: "a".to_owned(),
                },
                text: Some("a".to_owned()),
                state: WireKeyState::Pressed,
                modifiers: WireModifiers {
                    shift: false,
                    control: false,
                    alt: false,
                    super_key: false,
                },
                repeat: false,
            }),
            |_| true,
        );

        assert!(matches!(queue.try_pop(), NotificationPoll::Overloaded));
        assert_eq!(
            notifications.metrics(),
            ClpNotificationMetrics {
                overloaded_sessions: 1,
                ..ClpNotificationMetrics::default()
            }
        );
    }

    #[test]
    fn authenticated_client_completes_the_framed_handshake() {
        let server = ClpServer::start(CapabilitySet::from_capabilities(&[
            Capability::Scene,
            Capability::Input,
        ]))
        .unwrap();
        let mut client = connect(&server);

        send_client(&mut client, hello(&server, vec!["scene", "timing"]));
        let init = receive_host(&mut client);
        assert_eq!(init["type"], "init");
        assert_eq!(
            init["protocol"],
            json!({
                "major": crate::protocol::CLP_PROTOCOL_VERSION.major(),
                "minor": crate::protocol::CLP_PROTOCOL_VERSION.minor(),
            })
        );
        assert_eq!(init["capabilities"], json!(["scene"]));
        assert_eq!(
            init["context"],
            json!({ "arguments": [], "initial_commands": [] })
        );

        send_client(&mut client, WireClientMessage::InitComplete);
        let start = receive_host(&mut client);
        assert_eq!(start["type"], "start");
        assert_eq!(start["session_id"], init["session_id"]);

        send_client(&mut client, WireClientMessage::Started);
        wait_for_session_count(&server, 1);
        send_client(&mut client, WireClientMessage::Close);
        let closed = receive_host(&mut client);
        assert_eq!(closed["type"], "closed");
        assert_eq!(closed["reason"], json!({ "kind": "client_requested" }));
        wait_for_session_count(&server, 0);
    }

    #[test]
    fn delivers_input_timing_and_resize_messages_after_start() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["input", "timing"]));
        let init = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);
        let session_id = wait_for_session_started(&server);

        server.activate_session(session_id, 900, 600, 1.5);
        let resize = receive_host(&mut client);
        assert_eq!(resize["type"], "resize");
        assert_eq!(resize["physical_width"], 900);
        assert_eq!(resize["physical_height"], 600);
        assert_eq!(resize["logical_width"], 600.0);
        assert_eq!(resize["logical_height"], 400.0);
        assert_eq!(resize["scale_factor"], 1.5);

        send_client(
            &mut client,
            WireClientMessage::Subscribe {
                subscriptions: vec![WireSubscription::RawInput, WireSubscription::Timing],
            },
        );
        let subscribed = receive_host(&mut client);
        assert_eq!(subscribed["type"], "subscribed");
        assert_eq!(subscribed["subscriptions"], json!(["raw_input", "timing"]));

        server.send_keyboard_event(
            session_id,
            &KeyEvent {
                key: Key::Character("a".to_owned()),
                text: Some("a".to_owned()),
                state: KeyState::Pressed,
                modifiers: Modifiers::default(),
                repeat: false,
            },
        );
        server.broadcast_tick(Duration::from_millis(16));
        server.broadcast_frame(Duration::from_millis(8));

        let mut host_reader = FrameReader::default();
        let event = receive_host_with_reader(&mut client, &mut host_reader);
        assert_eq!(event["type"], "event");
        assert_eq!(event["session_id"], init["session_id"]);
        assert_eq!(
            event["event"],
            json!({
                "kind": "key",
                "key": { "kind": "character", "value": "a" },
                "text": "a",
                "state": "pressed",
                "modifiers": {
                    "shift": false,
                    "control": false,
                    "alt": false,
                    "super_key": false
                },
                "repeat": false
            })
        );

        let tick = receive_host_with_reader(&mut client, &mut host_reader);
        assert_eq!(tick["type"], "tick");
        assert_eq!(tick["dt_ns"], "16000000");

        let frame = receive_host_with_reader(&mut client, &mut host_reader);
        assert_eq!(frame["type"], "frame");
        assert_eq!(frame["dt_ns"], "8000000");

        send_client(&mut client, WireClientMessage::Close);
        let _ = receive_host(&mut client);
    }

    #[test]
    fn delivers_targeted_scene_events_with_owned_node_handles() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["input"]));
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);
        let session_id = wait_for_session_started(&server);
        server.activate_session(session_id, 900, 600, 1.5);
        let _ = receive_host(&mut client);
        let mut scene = crate::scene::Scene::new();
        let node = scene.create_root(crate::scene::NodeProperties::default());

        server.broadcast_scene_event(
            &SceneEvent::PointerEnter {
                target: node,
                position: Vec2 { x: 12.0, y: 24.0 },
                modifiers: Modifiers::default(),
            },
            |candidate_session, candidate_node| {
                (candidate_session == session_id && candidate_node == node)
                    .then_some(NodeHandle::new(41))
            },
        );

        let event = receive_host(&mut client);
        assert_eq!(event["type"], "event");
        assert_eq!(
            event["event"],
            json!({
                "kind": "scene",
                "event": {
                    "kind": "pointer_enter",
                    "target": "node-41",
                    "position": { "x": 12.0, "y": 24.0 },
                    "modifiers": {
                        "shift": false,
                        "control": false,
                        "alt": false,
                        "super_key": false
                    }
                }
            })
        );

        send_client(&mut client, WireClientMessage::Close);
        let _ = receive_host(&mut client);
    }

    #[test]
    fn delivers_overlap_events_to_sessions_with_the_overlap_capability() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["overlap"]));
        let init = receive_host(&mut client);
        assert_eq!(init["capabilities"], json!(["overlap"]));
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);
        let session_id = wait_for_session_started(&server);
        server.activate_session(session_id, 900, 600, 1.5);

        let mut scene = crate::scene::Scene::new();
        let first = scene.create_root(crate::scene::NodeProperties::default());
        let second = scene.create_root(crate::scene::NodeProperties::default());
        server.broadcast_scene_event(
            &SceneEvent::OverlapEnter { first, second },
            |candidate_session, candidate_node| {
                (candidate_session == session_id && candidate_node == first)
                    .then_some(NodeHandle::new(41))
                    .or_else(|| {
                        (candidate_session == session_id && candidate_node == second)
                            .then_some(NodeHandle::new(42))
                    })
            },
        );

        let event = receive_host(&mut client);
        assert_eq!(event["type"], "event");
        assert_eq!(
            event["event"],
            json!({
                "kind": "scene",
                "event": {
                    "kind": "overlap_enter",
                    "first": "node-41",
                    "second": "node-42"
                }
            })
        );

        send_client(&mut client, WireClientMessage::Close);
        let _ = receive_host(&mut client);
    }

    #[test]
    fn delivers_external_drop_batches_only_to_the_owning_opted_in_session() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["external_drop"]));
        let init = receive_host(&mut client);
        assert_eq!(init["capabilities"], json!(["external_drop"]));
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);
        let session_id = wait_for_session_started(&server);
        server.activate_session(session_id, 900, 600, 1.5);

        let mut scene = crate::scene::Scene::new();
        let target = scene.create_root(crate::scene::NodeProperties::default());
        server.broadcast_scene_event(
            &SceneEvent::ExternalDrop {
                target,
                position: Vec2 { x: 12.0, y: 24.0 },
                files: vec![
                    crate::runtime::ExternalFile {
                        uri: "file:///tmp/a%20file.txt".to_owned(),
                    },
                    crate::runtime::ExternalFile {
                        uri: "file:///tmp/second%20file.txt".to_owned(),
                    },
                ],
            },
            |candidate_session, candidate_node| {
                (candidate_session == session_id && candidate_node == target)
                    .then_some(NodeHandle::new(41))
            },
        );

        let event = receive_host(&mut client);
        assert_eq!(
            event["event"],
            json!({
                "kind": "scene",
                "event": {
                    "kind": "external_drop",
                    "target": "node-41",
                    "position": { "x": 12.0, "y": 24.0 },
                    "files": [
                        { "uri": "file:///tmp/a%20file.txt" },
                        { "uri": "file:///tmp/second%20file.txt" }
                    ]
                }
            })
        );

        send_client(&mut client, WireClientMessage::Close);
        let _ = receive_host(&mut client);
    }

    #[test]
    fn routes_external_drop_cancellation_only_to_the_target_owner() {
        let notifications = SessionNotifications::default();
        let mut sessions = SessionManager::new(CapabilitySet::ALL);
        let hello = ClientHello {
            protocol_version: crate::protocol::CLP_PROTOCOL_VERSION,
            requested_capabilities: CapabilitySet::ALL,
        };
        let owner = sessions
            .accept(hello.clone(), ProgramInitContext::default())
            .unwrap()
            .0;
        let other = sessions
            .accept(hello, ProgramInitContext::default())
            .unwrap()
            .0;
        let owner_queue = notifications.register(
            owner,
            CapabilitySet::from_capabilities(&[Capability::ExternalDrop]),
        );
        let other_queue = notifications.register(
            other,
            CapabilitySet::from_capabilities(&[Capability::ExternalDrop]),
        );
        assert!(notifications.activate(owner, resize_notification(1, 1, 1.0)));
        assert!(notifications.activate(other, resize_notification(1, 1, 1.0)));

        let mut scene = crate::scene::Scene::new();
        let target = scene.create_root(crate::scene::NodeProperties::default());
        notifications.broadcast_scene(
            &SceneEvent::ExternalDropLeave {
                target,
                position: Vec2 { x: 12.0, y: 24.0 },
            },
            |candidate_session, candidate_node| {
                (candidate_session == owner && candidate_node == target)
                    .then_some(NodeHandle::new(41))
            },
        );

        assert!(matches!(
            next_notification(&owner_queue),
            HostNotification::Event(WireEvent::Scene {
                event: WireSceneEvent::ExternalDropLeave { target, position }
            }) if target == "node-41" && position.x == 12.0 && position.y == 24.0
        ));
        assert!(matches!(other_queue.try_pop(), NotificationPoll::Empty));
    }

    #[test]
    fn withholds_external_drop_when_the_host_backend_cannot_deliver_it() {
        let server =
            ClpServer::start(CapabilitySet::ALL.without(Capability::ExternalDrop)).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["scene", "external_drop"]));
        let init = receive_host(&mut client);
        assert_eq!(init["capabilities"], json!(["scene"]));
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);
        let _ = wait_for_session_started(&server);

        send_client(
            &mut client,
            WireClientMessage::Command {
                request_id: "unsupported-external-drop-target".to_owned(),
                command: Box::new(WireSceneCommand::CreateNode {
                    parent: None,
                    layer: Some(WireSceneLayer::Overlay),
                    node: WireNodeInput::Empty {
                        properties: WireNodeProperties {
                            x: 0.0,
                            y: 0.0,
                            width: 10.0,
                            height: 10.0,
                            visible: true,
                            clip_children: false,
                            z_index: 0,
                            overlap_enabled: false,
                            external_drop_enabled: true,
                            input_passthrough: false,
                        },
                    },
                }),
            },
        );
        let error = receive_host(&mut client);
        assert_eq!(error["type"], "command_error");
        assert_eq!(error["code"], "capability_denied");

        send_client(&mut client, WireClientMessage::Close);
        let _ = receive_host(&mut client);
    }

    #[test]
    fn scene_drag_events_do_not_expose_foreign_node_handles() {
        let mut scene = crate::scene::Scene::new();
        let source = scene.create_root(crate::scene::NodeProperties::default());
        let target = scene.create_root(crate::scene::NodeProperties::default());
        let event = WireSceneEvent::from_scene_event(
            &SceneEvent::Drop {
                source,
                target,
                position: Vec2 { x: 12.0, y: 24.0 },
                modifiers: Modifiers::default(),
            },
            |node| (node == target).then_some("node-8".to_owned()),
        )
        .unwrap();

        assert_eq!(
            serde_json::to_value(event).unwrap(),
            json!({
                "kind": "drop",
                "source": null,
                "target": "node-8",
                "position": { "x": 12.0, "y": 24.0 },
                "modifiers": {
                    "shift": false,
                    "control": false,
                    "alt": false,
                    "super_key": false
                }
            })
        );
    }

    #[test]
    fn serializes_terminal_and_custom_engine_events() {
        let terminal =
            WireEvent::from_engine_event(&EngineEvent::Terminal(TerminalEvent::Exited)).unwrap();
        let custom =
            WireEvent::from_engine_event(&EngineEvent::Custom(crate::runtime::CustomEvent {
                name: "refresh".to_owned(),
                payload: vec![1, 2, 3],
            }))
            .unwrap();

        assert_eq!(
            serde_json::to_value(terminal).unwrap(),
            json!({ "kind": "terminal", "state": "exited" })
        );
        assert_eq!(
            serde_json::to_value(custom).unwrap(),
            json!({ "kind": "custom", "name": "refresh", "payload": [1, 2, 3] })
        );
    }

    #[test]
    fn only_delivers_notifications_granted_by_capability() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["timing"]));
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);
        let session_id = wait_for_session_started(&server);

        server.activate_session(session_id, 900, 600, 1.5);
        send_client(
            &mut client,
            WireClientMessage::Subscribe {
                subscriptions: vec![WireSubscription::RawInput],
            },
        );
        let denied = receive_host(&mut client);
        assert_eq!(denied["type"], "error");
        assert_eq!(denied["code"], "capability_denied");

        send_client(
            &mut client,
            WireClientMessage::Subscribe {
                subscriptions: vec![WireSubscription::Timing],
            },
        );
        let subscribed = receive_host(&mut client);
        assert_eq!(subscribed["type"], "subscribed");

        server.broadcast_event(&EngineEvent::Focus(FocusEvent::Gained));
        server.broadcast_tick(Duration::from_millis(16));

        let tick = receive_host(&mut client);
        assert_eq!(tick["type"], "tick");
        assert_eq!(tick["dt_ns"], "16000000");

        send_client(&mut client, WireClientMessage::Close);
        let _ = receive_host(&mut client);
    }

    #[test]
    fn forwards_rgba8_image_uploads_to_the_host_thread() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["images"]));
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);

        send_client(
            &mut client,
            WireClientMessage::Command {
                request_id: "image-request".to_owned(),
                command: Box::new(WireSceneCommand::UploadImage {
                    source: WireImageSource::Rgba8 {
                        width: 1,
                        height: 1,
                    },
                }),
            },
        );
        write_binary_frame(&mut client, &[1, 2, 3, 4]).unwrap();

        let request = wait_for_host_command(&server);
        assert_eq!(request.request_id, "image-request");
        assert_eq!(
            request.command,
            ClpSceneCommand::UploadImage {
                image: ImageUpload::Rgba8 {
                    width: 1,
                    height: 1,
                    pixels: vec![1, 2, 3, 4],
                },
            }
        );
        request.respond(HostCommandResponse::Success(ClpSceneCommandResult::Image(
            ImageHandle::new(7),
        )));

        let result = receive_host(&mut client);
        assert_eq!(result["type"], "result");
        assert_eq!(result["request_id"], "image-request");
        assert_eq!(
            result["result"],
            json!({ "kind": "image", "image": "image-7" })
        );

        send_client(&mut client, WireClientMessage::Close);
        let _ = receive_host(&mut client);
    }

    #[test]
    fn forwards_gif_image_uploads_and_requires_the_images_capability() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["images"]));
        let init = receive_host(&mut client);
        assert_eq!(init["capabilities"], json!(["images"]));
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);

        send_client(
            &mut client,
            WireClientMessage::Command {
                request_id: "gif-image-request".to_owned(),
                command: Box::new(WireSceneCommand::UploadImage {
                    source: WireImageSource::Gif { byte_length: 3 },
                }),
            },
        );
        write_binary_frame(&mut client, &[7, 8, 9]).unwrap();
        let request = wait_for_host_command(&server);
        assert_eq!(
            request.command,
            ClpSceneCommand::UploadImage {
                image: ImageUpload::Gif {
                    bytes: vec![7, 8, 9],
                },
            }
        );
        request.respond(HostCommandResponse::Success(ClpSceneCommandResult::Image(
            ImageHandle::new(7),
        )));
        let result = receive_host(&mut client);
        assert_eq!(
            result["result"],
            json!({ "kind": "image", "image": "image-7" })
        );
        send_client(&mut client, WireClientMessage::Close);
        let _ = receive_host(&mut client);

        let mut denied_client = connect(&server);
        send_client(&mut denied_client, hello(&server, vec![]));
        let _ = receive_host(&mut denied_client);
        send_client(&mut denied_client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut denied_client);
        send_client(&mut denied_client, WireClientMessage::Started);
        send_client(
            &mut denied_client,
            WireClientMessage::Command {
                request_id: "denied-gif-image".to_owned(),
                command: Box::new(WireSceneCommand::UploadImage {
                    source: WireImageSource::Gif { byte_length: 1 },
                }),
            },
        );
        write_binary_frame(&mut denied_client, &[0]).unwrap();
        let denied = receive_host(&mut denied_client);
        assert_eq!(denied["type"], "command_error");
        assert_eq!(denied["code"], "capability_denied");
    }

    #[test]
    fn rejects_commands_outside_the_negotiated_capabilities() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["scene"]));
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);

        send_client(
            &mut client,
            WireClientMessage::Command {
                request_id: "denied-upload".to_owned(),
                command: Box::new(WireSceneCommand::UploadImage {
                    source: WireImageSource::Rgba8 {
                        width: 1,
                        height: 1,
                    },
                }),
            },
        );
        write_binary_frame(&mut client, &[0; 4]).unwrap();

        let error = receive_host(&mut client);
        assert_eq!(error["type"], "command_error");
        assert_eq!(error["request_id"], "denied-upload");
        assert_eq!(error["code"], "capability_denied");
        assert!(
            !server
                .drain_host_actions()
                .into_iter()
                .any(|action| matches!(action, HostAction::Command(_)))
        );
    }

    #[test]
    fn requires_external_drop_capability_to_opt_in_a_scene_node() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["scene"]));
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);

        send_client(
            &mut client,
            WireClientMessage::Command {
                request_id: "denied-external-drop-target".to_owned(),
                command: Box::new(WireSceneCommand::CreateNode {
                    parent: None,
                    layer: Some(WireSceneLayer::Overlay),
                    node: WireNodeInput::Empty {
                        properties: WireNodeProperties {
                            x: 0.0,
                            y: 0.0,
                            width: 10.0,
                            height: 10.0,
                            visible: true,
                            clip_children: false,
                            z_index: 0,
                            overlap_enabled: false,
                            external_drop_enabled: true,
                            input_passthrough: false,
                        },
                    },
                }),
            },
        );

        let error = receive_host(&mut client);
        assert_eq!(error["type"], "command_error");
        assert_eq!(error["code"], "capability_denied");
        assert!(
            !server
                .drain_host_actions()
                .into_iter()
                .any(|action| matches!(action, HostAction::Command(_)))
        );

        send_client(&mut client, WireClientMessage::Close);
        let _ = receive_host(&mut client);
    }

    #[test]
    fn creates_text_nodes_and_reports_intrinsic_metrics() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["scene", "text"]));
        let init = receive_host(&mut client);
        assert_eq!(init["capabilities"], json!(["scene", "text"]));
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);

        send_client(
            &mut client,
            WireClientMessage::Command {
                request_id: "text-root".to_owned(),
                command: Box::new(WireSceneCommand::CreateNode {
                    parent: None,
                    layer: Some(WireSceneLayer::Overlay),
                    node: WireNodeInput::Text {
                        properties: WireTextNodeProperties {
                            x: 24.0,
                            y: 48.0,
                            visible: true,
                            clip_children: false,
                            z_index: 0,
                            overlap_enabled: false,
                            external_drop_enabled: false,
                            input_passthrough: false,
                        },
                        text: "Hello".to_owned(),
                        style: WireTextStyle::default(),
                    },
                }),
            },
        );
        let request = wait_for_host_command(&server);
        assert_eq!(request.request_id, "text-root");
        assert_eq!(
            request.command,
            ClpSceneCommand::CreateNode {
                parent: None,
                layer: Some(SceneLayer::Overlay),
                node: NodeInput::Text {
                    properties: TextNodePropertiesInput {
                        x: 24.0,
                        y: 48.0,
                        visible: true,
                        clip_children: false,
                        z_index: 0,
                        overlap_enabled: false,
                        external_drop_enabled: false,
                        input_passthrough: false,
                    },
                    text: "Hello".to_owned(),
                    style: TextStyle::default(),
                },
            }
        );
        request.respond(HostCommandResponse::Success(ClpSceneCommandResult::Node {
            node: NodeHandle::new(7),
            text_metrics: Some(TextMetrics {
                columns: 5,
                rows: 1,
                width: 48.0,
                height: 20.0,
                cells: 5,
            }),
        }));

        let result = receive_host(&mut client);
        assert_eq!(result["type"], "result");
        assert_eq!(
            result["result"],
            json!({
                "kind": "node",
                "node": "node-7",
                "text_metrics": {
                    "columns": 5,
                    "rows": 1,
                    "width": 48.0,
                    "height": 20.0,
                }
            })
        );

        send_client(&mut client, WireClientMessage::Close);
        let _ = receive_host(&mut client);
    }

    #[test]
    fn forwards_atomic_node_changes_and_serializes_their_result() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["scene"]));
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);

        send_client(
            &mut client,
            WireClientMessage::Command {
                request_id: "move-node".to_owned(),
                command: Box::new(WireSceneCommand::SetNode {
                    node: "node-4".to_owned(),
                    changes: WireNodeChanges {
                        properties: Some(WireNodePropertiesPatch {
                            x: Some(48.0),
                            y: Some(72.0),
                            width: None,
                            height: None,
                            visible: None,
                            clip_children: None,
                            z_index: None,
                            overlap_enabled: None,
                            external_drop_enabled: None,
                            input_passthrough: Some(true),
                        }),
                        ..WireNodeChanges::default()
                    },
                }),
            },
        );
        let request = wait_for_host_command(&server);
        assert_eq!(request.request_id, "move-node");
        assert_eq!(
            request.command,
            ClpSceneCommand::SetNode {
                node: NodeHandle::new(4),
                changes: NodeChanges {
                    properties: Some(NodePropertiesPatch {
                        x: Some(48.0),
                        y: Some(72.0),
                        input_passthrough: Some(true),
                        ..NodePropertiesPatch::default()
                    }),
                    ..NodeChanges::default()
                },
            }
        );
        request.respond(HostCommandResponse::Success(
            ClpSceneCommandResult::NodeUpdated { text_metrics: None },
        ));
        let result = receive_host(&mut client);
        assert_eq!(result["result"], json!({ "kind": "node_updated" }));

        send_client(&mut client, WireClientMessage::Close);
        let _ = receive_host(&mut client);
    }

    #[test]
    fn forwards_scene_batches_and_serializes_their_atomic_result() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(
            &mut client,
            hello(&server, vec!["scene", "text", "scene_batch"]),
        );
        let init = receive_host(&mut client);
        assert_eq!(
            init["capabilities"],
            json!(["scene", "text", "scene_batch"])
        );
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);

        send_client(
            &mut client,
            WireClientMessage::Command {
                request_id: "build-panel".to_owned(),
                command: Box::new(WireSceneCommand::SceneBatch {
                    operations: vec![
                        WireSceneBatchOperation::Create {
                            alias: "panel".to_owned(),
                            parent: None,
                            layer: Some(WireSceneLayer::Overlay),
                            node: WireNodeInput::Rectangle {
                                properties: WireNodeProperties {
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
                        WireSceneBatchOperation::Create {
                            alias: "label".to_owned(),
                            parent: Some(WireSceneBatchNodeRef::Alias {
                                alias: "panel".to_owned(),
                            }),
                            layer: None,
                            node: WireNodeInput::Text {
                                properties: WireTextNodeProperties {
                                    x: 16.0,
                                    y: 24.0,
                                    visible: true,
                                    clip_children: false,
                                    z_index: 0,
                                    overlap_enabled: false,
                                    external_drop_enabled: false,
                                    input_passthrough: false,
                                },
                                text: "Ready".to_owned(),
                                style: WireTextStyle::default(),
                            },
                        },
                        WireSceneBatchOperation::Set {
                            node: WireSceneBatchNodeRef::Alias {
                                alias: "panel".to_owned(),
                            },
                            changes: WireSceneBatchNodeChanges {
                                properties: Some(WireNodePropertiesPatch {
                                    x: Some(20.0),
                                    y: None,
                                    width: None,
                                    height: None,
                                    visible: None,
                                    clip_children: Some(true),
                                    z_index: None,
                                    overlap_enabled: None,
                                    external_drop_enabled: None,
                                    input_passthrough: None,
                                }),
                                ..WireSceneBatchNodeChanges::default()
                            },
                        },
                    ],
                }),
            },
        );

        let request = wait_for_host_command(&server);
        assert_eq!(request.request_id, "build-panel");
        assert_eq!(
            request.command,
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
                            properties: TextNodePropertiesInput {
                                x: 16.0,
                                y: 24.0,
                                visible: true,
                                clip_children: false,
                                z_index: 0,
                                overlap_enabled: false,
                                external_drop_enabled: false,
                                input_passthrough: false,
                            },
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
            }
        );
        request.respond(HostCommandResponse::Success(
            ClpSceneCommandResult::SceneBatch(SceneBatchResult {
                nodes: BTreeMap::from([
                    (
                        "label".to_owned(),
                        SceneBatchCreatedNode {
                            node: NodeHandle::new(2),
                            text_metrics: Some(TextMetrics {
                                columns: 5,
                                rows: 1,
                                width: 48.0,
                                height: 20.0,
                                cells: 5,
                            }),
                        },
                    ),
                    (
                        "panel".to_owned(),
                        SceneBatchCreatedNode {
                            node: NodeHandle::new(1),
                            text_metrics: None,
                        },
                    ),
                ]),
                operations: vec![
                    SceneBatchOperationResult::Created {
                        alias: "panel".to_owned(),
                    },
                    SceneBatchOperationResult::Created {
                        alias: "label".to_owned(),
                    },
                    SceneBatchOperationResult::Updated { text_metrics: None },
                ],
            }),
        ));

        let result = receive_host(&mut client);
        assert_eq!(result["type"], "result");
        assert_eq!(result["request_id"], "build-panel");
        assert_eq!(
            result["result"],
            json!({
                "kind": "scene_batch",
                "nodes": {
                    "label": {
                        "node": "node-2",
                        "text_metrics": {
                            "columns": 5,
                            "rows": 1,
                            "width": 48.0,
                            "height": 20.0,
                        },
                    },
                    "panel": { "node": "node-1" },
                },
                "operations": [
                    { "kind": "created", "alias": "panel" },
                    { "kind": "created", "alias": "label" },
                    { "kind": "updated" },
                ],
            })
        );

        send_client(&mut client, WireClientMessage::Close);
        let _ = receive_host(&mut client);
    }

    #[test]
    fn delivers_large_scene_batch_results_over_the_real_local_transport() {
        const LARGE_BATCH_NODE_COUNT: u64 = 16;

        let mut server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["scene", "scene_batch"]));
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);

        send_client(
            &mut client,
            WireClientMessage::Command {
                request_id: "large-scene-batch".to_owned(),
                command: Box::new(WireSceneCommand::SceneBatch {
                    operations: vec![WireSceneBatchOperation::Remove {
                        node: WireSceneBatchNodeRef::Handle("node-1".to_owned()),
                    }],
                }),
            },
        );

        let request = wait_for_host_command(&server);
        assert_eq!(request.request_id, "large-scene-batch");
        let batch = SceneBatchResult {
            nodes: (0..LARGE_BATCH_NODE_COUNT)
                .map(|index| {
                    let alias = format!("node-{index:02}-large-pipe");
                    (
                        alias.clone(),
                        SceneBatchCreatedNode {
                            node: NodeHandle::new(index + 1),
                            text_metrics: None,
                        },
                    )
                })
                .collect(),
            operations: (0..LARGE_BATCH_NODE_COUNT)
                .map(|index| SceneBatchOperationResult::Created {
                    alias: format!("node-{index:02}-large-pipe"),
                })
                .collect(),
        };
        let response = HostCommandResponse::Success(ClpSceneCommandResult::SceneBatch(batch));
        let response_frame = json_frame_bytes(&command_response_message(
            &request.request_id,
            response.clone(),
        ))
        .unwrap();
        assert!(
            response_frame.len() > MAX_RUNNING_FRAME_WRITE_CHUNK_BYTES,
            "the regression response must exceed the Windows named-pipe output buffer"
        );
        let (response_receiver, response_reader) = receive_host_async(&client);
        request.respond(response);

        let result = match response_receiver.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                response_reader.join().unwrap();
                panic!("the CLP server could not deliver the large response: {error}");
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                server.shutdown();
                response_reader.join().unwrap();
                panic!("the CLP server did not deliver the large response before the deadline");
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                response_reader.join().unwrap();
                panic!("the CLP response reader stopped before reporting a result");
            }
        };
        response_reader.join().unwrap();
        assert_eq!(result["type"], "result");
        assert_eq!(result["request_id"], "large-scene-batch");
        assert_eq!(result["result"]["kind"], "scene_batch");
        assert_eq!(
            result["result"]["nodes"].as_object().unwrap().len(),
            LARGE_BATCH_NODE_COUNT as usize
        );
        assert_eq!(
            result["result"]["operations"].as_array().unwrap().len(),
            LARGE_BATCH_NODE_COUNT as usize
        );
        assert_eq!(
            result["result"]["nodes"]["node-15-large-pipe"]["node"],
            "node-16"
        );

        send_client(&mut client, WireClientMessage::Close);
        let closed = receive_host(&mut client);
        assert_eq!(closed["type"], "closed");
        wait_for_session_count(&server, 0);
    }

    #[test]
    fn rejects_scene_batches_without_their_capability_or_with_an_oversized_envelope() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut denied_client = connect(&server);
        send_client(&mut denied_client, hello(&server, vec!["scene"]));
        let _ = receive_host(&mut denied_client);
        send_client(&mut denied_client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut denied_client);
        send_client(&mut denied_client, WireClientMessage::Started);
        send_client(
            &mut denied_client,
            WireClientMessage::Command {
                request_id: "denied-batch".to_owned(),
                command: Box::new(WireSceneCommand::SceneBatch {
                    operations: vec![WireSceneBatchOperation::Remove {
                        node: WireSceneBatchNodeRef::Handle("node-1".to_owned()),
                    }],
                }),
            },
        );
        let denied = receive_host(&mut denied_client);
        assert_eq!(denied["type"], "command_error");
        assert_eq!(denied["request_id"], "denied-batch");
        assert_eq!(denied["code"], "capability_denied");
        assert!(
            !server
                .drain_host_actions()
                .into_iter()
                .any(|action| matches!(action, HostAction::Command(_)))
        );
        send_client(&mut denied_client, WireClientMessage::Close);
        let _ = receive_host(&mut denied_client);

        let mut oversized_client = connect(&server);
        send_client(
            &mut oversized_client,
            hello(&server, vec!["scene", "scene_batch"]),
        );
        let _ = receive_host(&mut oversized_client);
        send_client(&mut oversized_client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut oversized_client);
        send_client(&mut oversized_client, WireClientMessage::Started);
        send_client(
            &mut oversized_client,
            WireClientMessage::Command {
                request_id: "oversized-batch".to_owned(),
                command: Box::new(WireSceneCommand::SceneBatch {
                    operations: vec![WireSceneBatchOperation::Create {
                        alias: "x".repeat(MAX_SCENE_BATCH_BYTES),
                        parent: None,
                        layer: Some(WireSceneLayer::Overlay),
                        node: WireNodeInput::Empty {
                            properties: WireNodeProperties {
                                x: 0.0,
                                y: 0.0,
                                width: 1.0,
                                height: 1.0,
                                visible: true,
                                clip_children: false,
                                z_index: 0,
                                overlap_enabled: false,
                                external_drop_enabled: false,
                                input_passthrough: false,
                            },
                        },
                    }],
                }),
            },
        );
        let oversized = receive_host(&mut oversized_client);
        assert_eq!(oversized["type"], "command_error");
        assert_eq!(oversized["request_id"], "oversized-batch");
        assert_eq!(oversized["code"], "resource_limit");
        assert!(
            !server
                .drain_host_actions()
                .into_iter()
                .any(|action| matches!(action, HostAction::Command(_)))
        );

        send_client(&mut oversized_client, WireClientMessage::Close);
        let _ = receive_host(&mut oversized_client);
    }

    #[test]
    fn forwards_window_metadata_with_tristate_fields_and_support_context() {
        let server = ClpServer::start_with_context_and_wake(
            CapabilitySet::ALL,
            ProgramInitContext::default().with_window_metadata_support(
                crate::protocol::WindowMetadataSupport {
                    icon_supported: true,
                },
            ),
            || {},
        )
        .unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["window_metadata"]));
        let init = receive_host(&mut client);
        assert_eq!(init["capabilities"], json!(["window_metadata"]));
        assert_eq!(
            init["context"]["window_metadata"],
            json!({ "icon_supported": true })
        );
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);

        send_client(
            &mut client,
            WireClientMessage::Command {
                request_id: "window-title".to_owned(),
                command: Box::new(WireSceneCommand::SetWindowMetadata {
                    title: WireMetadataField::Set("CLP metadata probe".to_owned()),
                    icon: WireMetadataField::Clear,
                }),
            },
        );
        let request = wait_for_host_command(&server);
        assert_eq!(
            request.command,
            ClpSceneCommand::SetWindowMetadata {
                metadata: WindowMetadataPatch {
                    title: MetadataField::Set("CLP metadata probe".to_owned()),
                    icon: MetadataField::Clear,
                },
            }
        );
        request.respond(HostCommandResponse::Success(
            ClpSceneCommandResult::WindowMetadata {
                icon_supported: true,
            },
        ));
        let result = receive_host(&mut client);
        assert_eq!(
            result["result"],
            json!({ "kind": "window_metadata", "icon_supported": true })
        );

        send_client(&mut client, WireClientMessage::Close);
        let _ = receive_host(&mut client);
    }

    #[test]
    fn withholds_window_metadata_context_from_a_v2_0_client() {
        let server = ClpServer::start_with_context_and_wake(
            CapabilitySet::ALL,
            ProgramInitContext::default().with_window_metadata_support(
                crate::protocol::WindowMetadataSupport {
                    icon_supported: true,
                },
            ),
            || {},
        )
        .unwrap();
        let mut client = connect(&server);
        send_client(
            &mut client,
            WireClientMessage::Hello {
                protocol: WireProtocolVersion { major: 2, minor: 0 },
                token: server.environment().authentication_token().to_owned(),
                capabilities: vec![WireCapability::WindowMetadata],
            },
        );

        let init = receive_host(&mut client);
        assert_eq!(init["capabilities"], json!([]));
        assert!(init["context"].get("window_metadata").is_none());
    }

    #[test]
    fn distinguishes_omitted_and_null_window_metadata_fields() {
        let preserve_icon: WireSceneCommand = serde_json::from_value(json!({
            "op": "set_window_metadata",
            "title": "Title only"
        }))
        .unwrap();
        let WireSceneCommand::SetWindowMetadata { title, icon } = preserve_icon else {
            unreachable!();
        };
        assert!(matches!(title, WireMetadataField::Set(value) if value == "Title only"));
        assert!(matches!(icon, WireMetadataField::Preserve));

        let clear: WireSceneCommand = serde_json::from_value(json!({
            "op": "set_window_metadata",
            "title": null,
            "icon": null
        }))
        .unwrap();
        let WireSceneCommand::SetWindowMetadata { title, icon } = clear else {
            unreachable!();
        };
        assert!(matches!(title, WireMetadataField::Clear));
        assert!(matches!(icon, WireMetadataField::Clear));
    }

    #[test]
    fn rejects_window_metadata_without_its_negotiated_capability() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec![]));
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);

        send_client(
            &mut client,
            WireClientMessage::Command {
                request_id: "denied-window-metadata".to_owned(),
                command: Box::new(WireSceneCommand::SetWindowMetadata {
                    title: WireMetadataField::Set("Denied".to_owned()),
                    icon: WireMetadataField::Preserve,
                }),
            },
        );
        let error = receive_host(&mut client);
        assert_eq!(error["type"], "command_error");
        assert_eq!(error["code"], "capability_denied");
        assert!(
            !server
                .drain_host_actions()
                .into_iter()
                .any(|action| matches!(action, HostAction::Command(_)))
        );

        send_client(&mut client, WireClientMessage::Close);
        let _ = receive_host(&mut client);
    }

    #[test]
    fn forwards_alpha_image_contact_and_input_passthrough() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["scene", "images"]));
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);

        send_client(
            &mut client,
            WireClientMessage::Command {
                request_id: "alpha-image".to_owned(),
                command: Box::new(WireSceneCommand::CreateNode {
                    parent: None,
                    layer: Some(WireSceneLayer::Overlay),
                    node: WireNodeInput::Image {
                        properties: WireNodeProperties {
                            x: 1.0,
                            y: 2.0,
                            width: 30.0,
                            height: 40.0,
                            visible: true,
                            clip_children: false,
                            z_index: 0,
                            overlap_enabled: false,
                            external_drop_enabled: false,
                            input_passthrough: true,
                        },
                        image: "image-7".to_owned(),
                        opacity: 1.0,
                        contact_shape: WireImageContactShape::Alpha { threshold: 1 },
                    },
                }),
            },
        );
        let request = wait_for_host_command(&server);
        assert_eq!(
            request.command,
            ClpSceneCommand::CreateNode {
                parent: None,
                layer: Some(SceneLayer::Overlay),
                node: NodeInput::Image {
                    properties: NodePropertiesInput {
                        x: 1.0,
                        y: 2.0,
                        width: 30.0,
                        height: 40.0,
                        visible: true,
                        clip_children: false,
                        z_index: 0,
                        overlap_enabled: false,
                        external_drop_enabled: false,
                        input_passthrough: true,
                    },
                    image: ImageHandle::new(7),
                    opacity: 1.0,
                    contact_shape: ImageContactShape::alpha(),
                },
            }
        );
        request.respond(HostCommandResponse::Success(ClpSceneCommandResult::Node {
            node: NodeHandle::new(4),
            text_metrics: None,
        }));
        let result = receive_host(&mut client);
        assert_eq!(
            result["result"],
            json!({ "kind": "node", "node": "node-4" })
        );

        send_client(&mut client, WireClientMessage::Close);
        let _ = receive_host(&mut client);
    }

    #[test]
    fn requires_the_text_capability_and_related_opt_ins_for_text_nodes() {
        let text = WireSceneCommand::CreateNode {
            parent: None,
            layer: Some(WireSceneLayer::Overlay),
            node: WireNodeInput::Text {
                properties: WireTextNodeProperties {
                    x: 0.0,
                    y: 0.0,
                    visible: true,
                    clip_children: false,
                    z_index: 0,
                    overlap_enabled: true,
                    external_drop_enabled: true,
                    input_passthrough: false,
                },
                text: "x".to_owned(),
                style: WireTextStyle::default(),
            },
        };
        let scene_and_text =
            CapabilitySet::from_capabilities(&[Capability::Scene, Capability::Text]);
        assert!(!wire_command_is_granted(&text, scene_and_text));
        let all = CapabilitySet::from_capabilities(&[
            Capability::Scene,
            Capability::Text,
            Capability::Overlap,
            Capability::ExternalDrop,
        ]);
        assert!(wire_command_is_granted(&text, all));
    }

    #[test]
    fn forwards_overlap_queries_and_requires_the_overlap_capability() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["scene", "overlap"]));
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);

        send_client(
            &mut client,
            WireClientMessage::Command {
                request_id: "overlap-query".to_owned(),
                command: Box::new(WireSceneCommand::GetOverlaps {
                    node: "node-3".to_owned(),
                }),
            },
        );
        let request = wait_for_host_command(&server);
        assert_eq!(request.request_id, "overlap-query");
        assert_eq!(
            request.command,
            ClpSceneCommand::GetOverlaps {
                node: NodeHandle::new(3)
            }
        );
        request.respond(HostCommandResponse::Success(
            ClpSceneCommandResult::Overlaps(vec![NodeHandle::new(7), NodeHandle::new(9)]),
        ));

        let result = receive_host(&mut client);
        assert_eq!(result["type"], "result");
        assert_eq!(
            result["result"],
            json!({
                "kind": "overlaps",
                "nodes": ["node-7", "node-9"]
            })
        );
        send_client(&mut client, WireClientMessage::Close);
        let _ = receive_host(&mut client);

        let mut denied_client = connect(&server);
        send_client(&mut denied_client, hello(&server, vec!["scene"]));
        let _ = receive_host(&mut denied_client);
        send_client(&mut denied_client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut denied_client);
        send_client(&mut denied_client, WireClientMessage::Started);
        send_client(
            &mut denied_client,
            WireClientMessage::Command {
                request_id: "denied-overlap-query".to_owned(),
                command: Box::new(WireSceneCommand::GetOverlaps {
                    node: "node-3".to_owned(),
                }),
            },
        );
        let denied = receive_host(&mut denied_client);
        assert_eq!(denied["type"], "command_error");
        assert_eq!(denied["code"], "capability_denied");
    }

    #[test]
    fn rejects_an_invalid_token_without_creating_a_session() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        let message = WireClientMessage::Hello {
            protocol: WireProtocolVersion { major: 1, minor: 0 },
            token: "not-the-session-token".to_owned(),
            capabilities: vec![WireCapability::Scene],
        };

        write_frame(&mut client, &message).unwrap();
        let error = receive_host(&mut client);
        assert_eq!(error["type"], "error");
        assert_eq!(error["code"], "authentication_failed");
        wait_for_session_count(&server, 0);
    }

    #[test]
    fn disconnecting_a_running_client_reclaims_its_session() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["scene"]));
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);
        wait_for_session_count(&server, 1);

        drop(client);
        wait_for_session_count(&server, 0);
    }

    #[test]
    fn keeps_an_idle_running_client_connected() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["scene"]));
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);
        wait_for_session_count(&server, 1);

        thread::sleep(Duration::from_millis(100));
        assert_eq!(server.session_count().unwrap(), 1);

        send_client(&mut client, WireClientMessage::Close);
        let _ = receive_host(&mut client);
        wait_for_session_count(&server, 0);
    }

    #[test]
    fn shutdown_closes_running_clients_and_reclaims_their_sessions() {
        let mut server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["scene"]));
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);
        let _ = wait_for_session_started(&server);

        thread::scope(|scope| {
            let shutdown = scope.spawn(|| server.shutdown());
            let closing = receive_host(&mut client);
            assert_eq!(closing["type"], "closing");
            assert_eq!(closing["reason"], json!({ "kind": "host_shutdown" }));
            send_client(&mut client, WireClientMessage::Close);
            shutdown.join().unwrap();
        });

        let closed = receive_host(&mut client);
        assert_eq!(closed["type"], "closed");
        assert_eq!(closed["reason"], json!({ "kind": "host_shutdown" }));
        assert_eq!(server.session_count().unwrap(), 0);
    }

    #[test]
    fn limits_connections_that_have_not_completed_the_handshake() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut pending_clients = Vec::with_capacity(MAX_PENDING_HANDSHAKES);
        for _ in 0..MAX_PENDING_HANDSHAKES {
            let mut client = connect(&server);
            client.write_all(&[0]).unwrap();
            client.flush().unwrap();
            pending_clients.push(client);
        }

        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if server.pending_handshake_count() == MAX_PENDING_HANDSHAKES {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(server.pending_handshake_count(), MAX_PENDING_HANDSHAKES);

        match try_connect(&server) {
            Ok(mut rejected_client) => {
                let setup_result = (|| -> io::Result<()> {
                    rejected_client.write_all(&[0])?;
                    rejected_client.flush()?;
                    rejected_client.set_nonblocking(true)
                })();
                match setup_result {
                    Ok(()) => {
                        let disconnected: FrameRead<Value> = FrameReader::default()
                            .read_next(
                                &mut rejected_client,
                                Some(Instant::now() + Duration::from_secs(1)),
                                None,
                            )
                            .unwrap();
                        assert!(matches!(disconnected, FrameRead::Disconnected));
                    }
                    Err(error) => assert!(connection_rejected_by_limit(&error)),
                }
            }
            Err(error) => assert!(connection_rejected_by_limit(&error)),
        }

        drop(pending_clients);
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if server.pending_handshake_count() == 0 {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(server.pending_handshake_count(), 0);
    }

    #[cfg(target_os = "windows")]
    fn connection_rejected_by_limit(error: &io::Error) -> bool {
        error.raw_os_error() == Some(231)
    }

    #[cfg(not(target_os = "windows"))]
    fn connection_rejected_by_limit(_: &io::Error) -> bool {
        false
    }

    #[test]
    fn rejects_frames_larger_than_the_protocol_limit() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        let length = u32::try_from(MAX_FRAME_BYTES + 1).unwrap();
        client.write_all(&length.to_be_bytes()).unwrap();
        client.flush().unwrap();

        let error = receive_host(&mut client);
        assert_eq!(error["type"], "error");
        assert_eq!(error["code"], "invalid_message");
        wait_for_session_count(&server, 0);
    }

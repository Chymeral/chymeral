use std::{
    env,
    error::Error,
    io::{Read, Write},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use chymeral::{
    program_scene::ClpSceneCommandResult,
    protocol::CapabilitySet,
    runtime::{
        EngineEvent, FocusEvent, Key, KeyEvent, KeyState, Modifiers, PointerEvent, TerminalEvent,
        Vec2,
    },
    transport::{
        CLP_ENDPOINT_ENV, CLP_TOKEN_ENV, ClpServer, HostAction, HostCommandResponse,
        MAX_FRAME_BYTES,
    },
};
use interprocess::local_socket::{
    GenericNamespaced, ToNsName, prelude::LocalSocketStream, traits::Stream as _,
};
use serde_json::{Value, json};

const CHILD_ENV: &str = "CHYMERAL_CLP_INHERITED_PROCESS_TEST_CHILD";

#[test]
fn inherited_process_completes_the_raw_clp_lifecycle() -> Result<(), Box<dyn Error>> {
    if env::var_os(CHILD_ENV).is_some() {
        return run_inherited_client();
    }

    let server = ClpServer::start(CapabilitySet::ALL)?;
    let executable = env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .arg("--exact")
        .arg("inherited_process_completes_the_raw_clp_lifecycle")
        .arg("--nocapture")
        .stderr(Stdio::piped())
        .env(CHILD_ENV, "1");
    for (name, value) in server.environment().variables() {
        command.env(name, value);
    }

    let mut child = command.spawn()?;
    let session_id = wait_for_started_session(&server)?;
    server.activate_session(session_id, 900, 600, 1.5);
    let ready_command = wait_for_host_command(&server)?;
    for index in 1..=128_u64 {
        server.send_raw_input_event(
            session_id,
            &EngineEvent::Pointer(PointerEvent::Move {
                position: Vec2 {
                    x: index as f64,
                    y: 24.0,
                },
                modifiers: Modifiers::default(),
            }),
        );
        server.broadcast_tick(Duration::from_nanos(index));
        server.broadcast_frame(Duration::from_nanos(index));
    }
    let metrics = server.notification_metrics();
    assert!(
        metrics.coalesced_notifications >= 375,
        "the continuous notification burst was not coalesced: {metrics:?}"
    );
    ready_command.respond(HostCommandResponse::Success(
        ClpSceneCommandResult::Acknowledged,
    ));

    let stress_complete = wait_for_host_command(&server)?;
    assert_eq!(stress_complete.request_id, "stress_complete");
    stress_complete.respond(HostCommandResponse::Success(
        ClpSceneCommandResult::Acknowledged,
    ));

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
    server.send_raw_input_event(
        session_id,
        &EngineEvent::Pointer(PointerEvent::Move {
            position: Vec2 { x: 12.0, y: 24.0 },
            modifiers: Modifiers::default(),
        }),
    );
    server.broadcast_event(&EngineEvent::Focus(FocusEvent::Gained));
    server.broadcast_event(&EngineEvent::Terminal(TerminalEvent::Exited));
    server.broadcast_tick(Duration::from_millis(16));
    server.broadcast_frame(Duration::from_millis(8));

    let status = child.wait()?;
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .ok_or("missing child stderr")?
        .read_to_string(&mut stderr)?;
    assert!(
        status.success(),
        "the inherited CLP client failed:\nstderr:\n{stderr}",
    );
    assert_eq!(server.session_count()?, 0);
    Ok(())
}

fn run_inherited_client() -> Result<(), Box<dyn Error>> {
    let endpoint = env::var(CLP_ENDPOINT_ENV)?;
    let token = env::var(CLP_TOKEN_ENV)?;
    let name = endpoint.to_ns_name::<GenericNamespaced>()?;
    let mut stream = LocalSocketStream::connect(name)?;

    write_json(
        &mut stream,
        &json!({
            "type": "hello",
            "protocol": { "major": 2, "minor": 0 },
            "token": token,
            "capabilities": ["scene", "input", "timing"]
        }),
    )?;
    let init = read_json(&mut stream)?;
    assert_eq!(init["type"], "init");
    assert_eq!(init["protocol"], json!({ "major": 2, "minor": 0 }));
    assert_eq!(init["capabilities"], json!(["scene", "input", "timing"]));

    write_json(&mut stream, &json!({ "type": "init_complete" }))?;
    let start = read_json(&mut stream)?;
    assert_eq!(start["type"], "start");
    assert_eq!(start["session_id"], init["session_id"]);

    write_json(&mut stream, &json!({ "type": "started" }))?;
    let resize = read_json(&mut stream)?;
    assert_eq!(resize["type"], "resize");
    assert_eq!(resize["physical_width"], 900);
    assert_eq!(resize["physical_height"], 600);
    assert_eq!(resize["logical_width"], 600.0);
    assert_eq!(resize["logical_height"], 400.0);
    assert_eq!(resize["scale_factor"], 1.5);

    write_json(
        &mut stream,
        &json!({
            "type": "subscribe",
            "subscriptions": ["raw_input", "terminal", "timing"]
        }),
    )?;
    let subscribed = read_json(&mut stream)?;
    assert_eq!(subscribed["type"], "subscribed");
    assert_eq!(
        subscribed["subscriptions"],
        json!(["raw_input", "terminal", "timing"])
    );

    write_json(
        &mut stream,
        &json!({
            "type": "command",
            "request_id": "ready",
            "command": {
                "op": "create_node",
                "node": {
                    "kind": "empty",
                    "properties": {
                        "x": 0.0,
                        "y": 0.0,
                        "width": 1.0,
                        "height": 1.0
                    }
                }
            }
        }),
    )?;
    let ready = read_json(&mut stream)?;
    assert_eq!(ready["type"], "result");
    assert_eq!(ready["request_id"], "ready");
    assert_eq!(ready["result"], json!({ "kind": "acknowledged" }));

    let mut saw_gap = false;
    let mut saw_latest_pointer = false;
    let mut saw_latest_tick = false;
    let mut saw_latest_frame = false;
    while !(saw_gap && saw_latest_pointer && saw_latest_tick && saw_latest_frame) {
        let message = read_json(&mut stream)?;
        match message["type"].as_str() {
            Some("delivery_gap") => {
                assert_eq!(message["categories"], json!(["raw_input", "timing"]));
                saw_gap = true;
            }
            Some("event") if message["event"]["kind"] == "pointer_move" => {
                assert_eq!(message["event"]["position"]["x"], json!(128.0));
                saw_latest_pointer = true;
            }
            Some("tick") => {
                assert_eq!(message["dt_ns"], "128");
                saw_latest_tick = true;
            }
            Some("frame") => {
                assert_eq!(message["dt_ns"], "128");
                saw_latest_frame = true;
            }
            other => return Err(format!("unexpected stress message: {other:?}: {message}").into()),
        }
    }

    write_json(
        &mut stream,
        &json!({
            "type": "command",
            "request_id": "stress_complete",
            "command": {
                "op": "create_node",
                "node": {
                    "kind": "empty",
                    "properties": {
                        "x": 1.0,
                        "y": 1.0,
                        "width": 1.0,
                        "height": 1.0
                    }
                }
            }
        }),
    )?;
    let stress_complete = read_json(&mut stream)?;
    assert_eq!(stress_complete["type"], "result");
    assert_eq!(stress_complete["request_id"], "stress_complete");

    let key = read_json(&mut stream)?;
    assert_eq!(key["type"], "event");
    assert_eq!(key["event"]["kind"], "key");
    assert_eq!(
        key["event"]["key"],
        json!({ "kind": "character", "value": "a" })
    );

    let pointer = read_json(&mut stream)?;
    assert_eq!(pointer["type"], "event");
    assert_eq!(
        pointer["event"],
        json!({
            "kind": "pointer_move",
            "position": { "x": 12.0, "y": 24.0 },
            "modifiers": {
                "shift": false,
                "control": false,
                "alt": false,
                "super_key": false
            }
        })
    );

    let focus = read_json(&mut stream)?;
    assert_eq!(focus["type"], "event");
    assert_eq!(
        focus["event"],
        json!({ "kind": "focus", "state": "gained" })
    );

    let terminal = read_json(&mut stream)?;
    assert_eq!(terminal["type"], "event");
    assert_eq!(
        terminal["event"],
        json!({ "kind": "terminal", "state": "exited" })
    );

    let tick = read_json(&mut stream)?;
    assert_eq!(tick["type"], "tick");
    assert_eq!(tick["dt_ns"], "16000000");

    let frame = read_json(&mut stream)?;
    assert_eq!(frame["type"], "frame");
    assert_eq!(frame["dt_ns"], "8000000");

    write_json(&mut stream, &json!({ "type": "close" }))?;
    let closed = read_json(&mut stream)?;
    assert_eq!(closed["type"], "closed");
    assert_eq!(closed["reason"], json!({ "kind": "client_requested" }));
    Ok(())
}

fn wait_for_started_session(
    server: &ClpServer,
) -> Result<chymeral::protocol::SessionId, Box<dyn Error>> {
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
            return Ok(session_id);
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err("the inherited CLP client did not enter the running state".into())
}

fn wait_for_host_command(
    server: &ClpServer,
) -> Result<chymeral::transport::HostCommandRequest, Box<dyn Error>> {
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
            return Ok(request);
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err("the inherited CLP client did not issue its ready command".into())
}

fn write_json(stream: &mut LocalSocketStream, message: &Value) -> Result<(), Box<dyn Error>> {
    let payload = serde_json::to_vec(message)?;
    let length = u32::try_from(payload.len() + 1)?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(b"J")?;
    stream.write_all(&payload)?;
    stream.flush()?;
    Ok(())
}

fn read_json(stream: &mut LocalSocketStream) -> Result<Value, Box<dyn Error>> {
    let mut length_bytes = [0_u8; 4];
    stream.read_exact(&mut length_bytes)?;
    let length = usize::try_from(u32::from_be_bytes(length_bytes))?;
    if length > MAX_FRAME_BYTES {
        return Err(format!("CLP frame exceeds the {MAX_FRAME_BYTES}-byte limit").into());
    }
    if length == 0 {
        return Err("CLP frame is missing its kind".into());
    }
    let mut kind = [0_u8; 1];
    stream.read_exact(&mut kind)?;
    if kind != *b"J" {
        return Err("expected a JSON CLP frame".into());
    }
    let mut payload = vec![0_u8; length - 1];
    stream.read_exact(&mut payload)?;
    Ok(serde_json::from_slice(&payload)?)
}

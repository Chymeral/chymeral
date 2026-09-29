use std::{
    env,
    error::Error,
    io::{Read, Write},
    process::Command,
};

use interprocess::local_socket::{
    GenericNamespaced, ToNsName, prelude::LocalSocketStream, traits::Stream as _,
};
use moditty::{
    protocol::CapabilitySet,
    transport::{MAX_FRAME_BYTES, MLP_ENDPOINT_ENV, MLP_TOKEN_ENV, MlpServer},
};
use serde_json::{Value, json};

const CHILD_ENV: &str = "MODITTY_MLP_INHERITED_PROCESS_TEST_CHILD";

#[test]
fn inherited_process_completes_the_raw_mlp_handshake() -> Result<(), Box<dyn Error>> {
    if env::var_os(CHILD_ENV).is_some() {
        return run_inherited_client();
    }

    let server = MlpServer::start(CapabilitySet::ALL)?;
    let executable = env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .arg("--exact")
        .arg("inherited_process_completes_the_raw_mlp_handshake")
        .arg("--nocapture")
        .env(CHILD_ENV, "1");
    for (name, value) in server.environment().variables() {
        command.env(name, value);
    }

    let output = command.output()?;
    assert!(
        output.status.success(),
        "the inherited MLP client failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(server.session_count()?, 0);
    Ok(())
}

fn run_inherited_client() -> Result<(), Box<dyn Error>> {
    let endpoint = env::var(MLP_ENDPOINT_ENV)?;
    let token = env::var(MLP_TOKEN_ENV)?;
    let name = endpoint.to_ns_name::<GenericNamespaced>()?;
    let mut stream = LocalSocketStream::connect(name)?;

    write_json(
        &mut stream,
        &json!({
            "type": "hello",
            "protocol": { "major": 1, "minor": 0 },
            "token": token,
            "capabilities": ["scene", "input"]
        }),
    )?;
    let init = read_json(&mut stream)?;
    assert_eq!(init["type"], "init");
    assert_eq!(init["capabilities"], json!(["scene", "input"]));

    write_json(&mut stream, &json!({ "type": "init_complete" }))?;
    let start = read_json(&mut stream)?;
    assert_eq!(start["type"], "start");
    assert_eq!(start["session_id"], init["session_id"]);

    write_json(&mut stream, &json!({ "type": "started" }))?;
    write_json(&mut stream, &json!({ "type": "close" }))?;
    let closed = read_json(&mut stream)?;
    assert_eq!(closed["type"], "closed");
    assert_eq!(closed["reason"], json!({ "kind": "client_requested" }));
    Ok(())
}

fn write_json(stream: &mut LocalSocketStream, message: &Value) -> Result<(), Box<dyn Error>> {
    let payload = serde_json::to_vec(message)?;
    let length = u32::try_from(payload.len())?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(&payload)?;
    stream.flush()?;
    Ok(())
}

fn read_json(stream: &mut LocalSocketStream) -> Result<Value, Box<dyn Error>> {
    let mut length_bytes = [0_u8; 4];
    stream.read_exact(&mut length_bytes)?;
    let length = usize::try_from(u32::from_be_bytes(length_bytes))?;
    if length > MAX_FRAME_BYTES {
        return Err(format!("MLP frame exceeds the {MAX_FRAME_BYTES}-byte limit").into());
    }
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload)?;
    Ok(serde_json::from_slice(&payload)?)
}

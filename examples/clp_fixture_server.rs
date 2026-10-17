use std::{
    env,
    error::Error,
    process::{Command, ExitStatus},
    thread,
    time::{Duration, Instant},
};

use chymeral::{
    program_scene::{ProgramSceneError, ProgramSceneResources},
    protocol::CapabilitySet,
    resources::TextureStore,
    scene::Scene,
    transport::{ClpServer, HostAction, HostCommandResponse},
};

const FIXTURE_TIMEOUT: Duration = Duration::from_secs(30);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);
const FIXTURE_TICK: Duration = Duration::from_millis(16);
const FIXTURE_TEXTURE_DIMENSION: u32 = 8_192;

fn main() -> Result<(), Box<dyn Error>> {
    let mut command = env::args_os().skip(1);
    let program = command
        .next()
        .ok_or("usage: cargo run --example clp_fixture_server -- <client command> [args...]")?;

    let mut server = ClpServer::start(CapabilitySet::ALL)?;
    let mut child_command = Command::new(program);
    child_command.args(command);
    for (name, value) in server.environment().variables() {
        child_command.env(name, value);
    }
    let mut child = child_command.spawn()?;

    let result = run_fixture(&server, &mut child);
    server.shutdown();
    result
}

fn run_fixture(server: &ClpServer, child: &mut std::process::Child) -> Result<(), Box<dyn Error>> {
    let mut resources = ProgramSceneResources::new(FIXTURE_TEXTURE_DIMENSION);
    let mut scene = Scene::new();
    let mut textures = TextureStore::new();
    let deadline = Instant::now() + FIXTURE_TIMEOUT;
    let mut child_status = None;
    let mut cleanup_deadline = None;
    let mut next_tick = Instant::now();

    loop {
        for action in server.drain_host_actions() {
            match action {
                HostAction::Command(request) => {
                    let (session_id, _, command, response_sender) = (*request).into_parts();
                    let response =
                        match resources.execute_clp(session_id, command, &mut scene, &mut textures)
                        {
                            Ok(result) => HostCommandResponse::Success(result),
                            Err(error) => HostCommandResponse::Error {
                                code: scene_error_code(&error),
                                message: error.to_string(),
                            },
                        };
                    let _ = response_sender.send(response);
                }
                HostAction::Cleanup { session_id } => {
                    resources.cleanup_session(session_id, &mut scene, &mut textures);
                }
                HostAction::SessionStarted { session_id } => {
                    server.activate_session(session_id, 1_024, 768, 1.0);
                }
            }
        }

        let now = Instant::now();
        if now >= next_tick {
            server.broadcast_tick(FIXTURE_TICK);
            server.broadcast_frame(FIXTURE_TICK);
            next_tick = now + FIXTURE_TICK;
        }

        if child_status.is_none() {
            child_status = child.try_wait()?;
            if child_status.is_some() {
                cleanup_deadline = Some(Instant::now() + CLEANUP_TIMEOUT);
            }
        }

        if let Some(status) = child_status {
            if server.session_count()? == 0 {
                return check_client_status(status);
            }
            if Instant::now() >= cleanup_deadline.expect("a completed child has a deadline") {
                return Err("the CLP fixture client exited without releasing its session".into());
            }
        }

        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("the CLP fixture client exceeded the 30-second deadline".into());
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn check_client_status(status: ExitStatus) -> Result<(), Box<dyn Error>> {
    if status.success() {
        Ok(())
    } else {
        Err(format!("the CLP fixture client exited with {status}").into())
    }
}

fn scene_error_code(error: &ProgramSceneError) -> &'static str {
    match error {
        ProgramSceneError::InvalidNodeHandle(_)
        | ProgramSceneError::InvalidImageHandle(_)
        | ProgramSceneError::InvalidTextureHandle(_)
        | ProgramSceneError::InvalidAnimatedImageHandle(_) => "invalid_handle",
        ProgramSceneError::HandleNotOwned => "not_owner",
        ProgramSceneError::ImageInUse(_)
        | ProgramSceneError::TextureInUse(_)
        | ProgramSceneError::AnimatedImageInUse(_) => "image_in_use",
        ProgramSceneError::TextureLimitExceeded { .. }
        | ProgramSceneError::SessionTextureLimitExceeded { .. }
        | ProgramSceneError::TextureDimensionLimitExceeded { .. }
        | ProgramSceneError::OverlapNodeLimitExceeded { .. }
        | ProgramSceneError::TextLimitExceeded { .. }
        | ProgramSceneError::SessionTextLimitExceeded { .. }
        | ProgramSceneError::SessionTextNodeLimitExceeded { .. } => "resource_limit",
        ProgramSceneError::Geometry(_)
        | ProgramSceneError::Image(_)
        | ProgramSceneError::Texture(_)
        | ProgramSceneError::AnimatedImage(_)
        | ProgramSceneError::Text(_) => "invalid_argument",
        ProgramSceneError::TextNodeRequired(_)
        | ProgramSceneError::IntrinsicTextBounds(_)
        | ProgramSceneError::TextContentRequiresTextCommands(_)
        | ProgramSceneError::NodeKindMismatch(_)
        | ProgramSceneError::InvalidNodeChange(_)
        | ProgramSceneError::Scene(_) => "invalid_node_change",
        ProgramSceneError::HandleExhausted => "host_capacity",
    }
}

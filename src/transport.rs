use std::{
    io::{self, Read, Write},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use getrandom::fill as fill_random;
use interprocess::local_socket::{
    GenericNamespaced, ListenerNonblockingMode, ListenerOptions, ToNsName,
    prelude::{LocalSocketListener, LocalSocketStream},
    traits::{Listener as _, Stream as _},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::protocol::{
    Capability, CapabilitySet, ClientHello, ClientMessage, HostMessage, ProgramFault,
    ProgramInitContext, ProgramPhase, ProtocolError, ProtocolVersion, SessionCloseReason,
    SessionId, SessionManager,
};
use crate::{
    program_scene::{
        ContentInput, MAX_TEXTURE_BYTES, NodeHandle, NodePropertiesInput, NodePropertiesPatch,
        SceneCommand, SceneCommandResult, TextureHandle,
    },
    resources::rgba8_byte_len,
    scene::SceneLayer,
};

pub const CLP_ENDPOINT_ENV: &str = "CHYMERAL_CLP_ENDPOINT";
pub const CLP_TOKEN_ENV: &str = "CHYMERAL_CLP_TOKEN";
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_PENDING_HANDSHAKES: usize = 16;
pub const MAX_PENDING_HOST_ACTIONS: usize = 64;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const LISTENER_POLL_INTERVAL: Duration = Duration::from_millis(10);
const ENDPOINT_BIND_ATTEMPTS: usize = 8;

#[derive(Clone)]
pub struct SessionEnvironment {
    endpoint: String,
    token: String,
}

impl SessionEnvironment {
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn authentication_token(&self) -> &str {
        &self.token
    }

    pub fn variables(&self) -> [(&'static str, &str); 2] {
        [
            (CLP_ENDPOINT_ENV, self.endpoint()),
            (CLP_TOKEN_ENV, self.authentication_token()),
        ]
    }
}

pub struct ClpServer {
    environment: SessionEnvironment,
    listener_thread: Option<thread::JoinHandle<()>>,
    manager: Arc<Mutex<SessionManager>>,
    control: Arc<ServerControl>,
    host_actions: Mutex<Receiver<HostAction>>,
    cleanup_actions: Mutex<Receiver<SessionId>>,
}

pub enum HostAction {
    Command(HostCommandRequest),
    Cleanup { session_id: SessionId },
}

pub struct HostCommandRequest {
    pub session_id: SessionId,
    pub request_id: String,
    pub command: SceneCommand,
    response: SyncSender<HostCommandResponse>,
}

impl HostCommandRequest {
    pub fn respond(self, response: HostCommandResponse) {
        let _ = self.response.send(response);
    }

    pub fn into_parts(
        self,
    ) -> (
        SessionId,
        String,
        SceneCommand,
        SyncSender<HostCommandResponse>,
    ) {
        (
            self.session_id,
            self.request_id,
            self.command,
            self.response,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostCommandResponse {
    Success(SceneCommandResult),
    Error { code: &'static str, message: String },
}

#[derive(Clone)]
struct HostActionBroker {
    sender: SyncSender<HostAction>,
    cleanup_sender: mpsc::Sender<SessionId>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl ClpServer {
    pub fn start(available_capabilities: CapabilitySet) -> Result<Self> {
        Self::start_with_wake(available_capabilities, || {})
    }

    pub fn start_with_wake(
        available_capabilities: CapabilitySet,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self> {
        let (environment, listener) = bind_listener()?;
        let control = Arc::new(ServerControl::default());
        let manager = Arc::new(Mutex::new(SessionManager::new(available_capabilities)));
        let (host_action_sender, host_actions) = mpsc::sync_channel(MAX_PENDING_HOST_ACTIONS);
        let (cleanup_sender, cleanup_actions) = mpsc::channel();
        let broker = HostActionBroker {
            sender: host_action_sender,
            cleanup_sender,
            wake: Arc::new(wake),
        };
        let listener_control = Arc::clone(&control);
        let listener_manager = Arc::clone(&manager);
        let listener_environment = environment.clone();
        let listener_broker = broker.clone();
        let listener_thread = thread::Builder::new()
            .name("chymeral-clp-listener".to_owned())
            .spawn(move || {
                serve_connections(
                    listener,
                    listener_environment,
                    listener_manager,
                    listener_control,
                    listener_broker,
                );
            })
            .context("could not start the CLP listener")?;

        Ok(Self {
            environment,
            listener_thread: Some(listener_thread),
            manager,
            control,
            host_actions: Mutex::new(host_actions),
            cleanup_actions: Mutex::new(cleanup_actions),
        })
    }

    pub fn environment(&self) -> &SessionEnvironment {
        &self.environment
    }

    pub fn session_count(&self) -> Result<usize> {
        self.manager
            .lock()
            .map(|manager| manager.session_count())
            .map_err(|_| anyhow!("the CLP session manager is unavailable"))
    }

    pub fn shutdown(&mut self) {
        self.control.request_shutdown();
        if let Some(listener_thread) = self.listener_thread.take() {
            let _ = listener_thread.join();
        }
        self.control.wait_for_handlers();
    }

    pub fn drain_host_actions(&self) -> Vec<HostAction> {
        let mut actions: Vec<HostAction> = self
            .host_actions
            .lock()
            .map(|receiver| std::iter::from_fn(|| receiver.try_recv().ok()).collect())
            .unwrap_or_default();
        if let Ok(receiver) = self.cleanup_actions.lock() {
            actions.extend(
                std::iter::from_fn(|| receiver.try_recv().ok())
                    .map(|session_id| HostAction::Cleanup { session_id }),
            );
        }
        actions
    }

    #[cfg(test)]
    fn pending_handshake_count(&self) -> usize {
        self.control.pending_handshake_count()
    }
}

impl HostActionBroker {
    fn submit(&self, request: HostCommandRequest) -> Result<()> {
        match self.sender.try_send(HostAction::Command(request)) {
            Ok(()) => {
                (self.wake)();
                Ok(())
            }
            Err(TrySendError::Full(_)) => Err(anyhow!("the host command queue is full")),
            Err(TrySendError::Disconnected(_)) => {
                Err(anyhow!("the host command queue is unavailable"))
            }
        }
    }

    fn cleanup(&self, session_id: SessionId) {
        if self.cleanup_sender.send(session_id).is_ok() {
            (self.wake)();
        }
    }
}

impl Drop for ClpServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Default)]
struct ServerControl {
    shutting_down: AtomicBool,
    state: Mutex<ServerControlState>,
    handlers_finished: Condvar,
}

#[derive(Default)]
struct ServerControlState {
    pending_handshakes: usize,
    handlers: usize,
}

impl ServerControl {
    fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Acquire)
    }

    fn try_register_connection(self: &Arc<Self>) -> Option<ConnectionGuard> {
        let mut state = self.lock_state();
        if self.is_shutting_down() || state.pending_handshakes >= MAX_PENDING_HANDSHAKES {
            return None;
        }
        state.pending_handshakes += 1;
        state.handlers += 1;
        Some(ConnectionGuard {
            control: Arc::clone(self),
            handshake_pending: true,
        })
    }

    fn request_shutdown(&self) {
        self.shutting_down.store(true, Ordering::Release);
    }

    fn wait_for_handlers(&self) {
        let mut state = self.lock_state();
        while state.handlers != 0 {
            state = match self.handlers_finished.wait(state) {
                Ok(state) => state,
                Err(poisoned) => poisoned.into_inner(),
            };
        }
    }

    #[cfg(test)]
    fn pending_handshake_count(&self) -> usize {
        self.lock_state().pending_handshakes
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, ServerControlState> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

struct ConnectionGuard {
    control: Arc<ServerControl>,
    handshake_pending: bool,
}

impl ConnectionGuard {
    fn complete_handshake(&mut self) {
        if !self.handshake_pending {
            return;
        }
        let mut state = self.control.lock_state();
        state.pending_handshakes -= 1;
        self.handshake_pending = false;
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let mut state = self.control.lock_state();
        if self.handshake_pending {
            state.pending_handshakes -= 1;
        }
        state.handlers -= 1;
        if state.handlers == 0 {
            self.control.handlers_finished.notify_all();
        }
    }
}

fn bind_listener() -> Result<(SessionEnvironment, LocalSocketListener)> {
    for _ in 0..ENDPOINT_BIND_ATTEMPTS {
        let environment = SessionEnvironment {
            endpoint: endpoint_name()?,
            token: random_token()?,
        };
        let name = environment
            .endpoint()
            .to_ns_name::<GenericNamespaced>()
            .context("could not construct the CLP endpoint name")?;
        match ListenerOptions::new()
            .name(name)
            .nonblocking(ListenerNonblockingMode::Accept)
            .create_sync()
        {
            Ok(listener) => return Ok((environment, listener)),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => continue,
            Err(error) => return Err(error).context("could not bind the local CLP endpoint"),
        }
    }
    Err(anyhow!(
        "could not allocate a unique local CLP endpoint after {ENDPOINT_BIND_ATTEMPTS} attempts"
    ))
}

fn endpoint_name() -> Result<String> {
    Ok(format!("chymeral-clp-{}", random_hex(16)?))
}

fn random_token() -> Result<String> {
    random_hex(32)
}

fn random_hex(byte_count: usize) -> Result<String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut bytes = vec![0_u8; byte_count];
    fill_random(&mut bytes).map_err(|error| {
        anyhow!("could not generate local CLP authentication material: {error}")
    })?;
    let mut result = String::with_capacity(byte_count * 2);
    for byte in bytes {
        result.push(char::from(HEX[usize::from(byte >> 4)]));
        result.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(result)
}

fn serve_connections(
    listener: LocalSocketListener,
    environment: SessionEnvironment,
    manager: Arc<Mutex<SessionManager>>,
    control: Arc<ServerControl>,
    broker: HostActionBroker,
) {
    while !control.is_shutting_down() {
        match listener.accept() {
            Ok(stream) => {
                let Some(connection_guard) = control.try_register_connection() else {
                    drop(stream);
                    continue;
                };
                let connection_environment = environment.clone();
                let connection_manager = Arc::clone(&manager);
                let connection_control = Arc::clone(&control);
                let connection_broker = broker.clone();
                if let Err(error) = thread::Builder::new()
                    .name("chymeral-clp-client".to_owned())
                    .spawn(move || {
                        handle_connection(
                            stream,
                            connection_environment,
                            connection_manager,
                            connection_control,
                            connection_broker,
                            connection_guard,
                        );
                    })
                {
                    eprintln!("Chymeral: could not serve a CLP client: {error}");
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(LISTENER_POLL_INTERVAL);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                if !control.is_shutting_down() {
                    eprintln!("Chymeral: local CLP listener error: {error}");
                    thread::sleep(LISTENER_POLL_INTERVAL);
                }
            }
        }
    }
}

fn handle_connection(
    mut stream: LocalSocketStream,
    environment: SessionEnvironment,
    manager: Arc<Mutex<SessionManager>>,
    control: Arc<ServerControl>,
    broker: HostActionBroker,
    mut connection_guard: ConnectionGuard,
) {
    if let Err(error) = stream.set_nonblocking(true) {
        eprintln!("Chymeral: could not configure nonblocking CLP handshake I/O: {error}");
        return;
    }
    let handshake_deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let mut frame_reader = FrameReader::default();

    let hello = match read_client_message(
        &mut stream,
        &mut frame_reader,
        Some(handshake_deadline),
        &control,
    ) {
        Ok(FrameRead::Message(WireClientMessage::Hello {
            protocol,
            token,
            capabilities,
        })) => WireHello {
            protocol,
            token,
            capabilities,
        },
        Ok(FrameRead::Message(_)) => {
            let _ = write_error(
                &mut stream,
                "expected_hello",
                "the first CLP message must be hello",
            );
            return;
        }
        Ok(FrameRead::Disconnected) => return,
        Ok(FrameRead::Shutdown) => return,
        Err(error) => {
            let _ = write_error(&mut stream, "invalid_message", &error.to_string());
            return;
        }
    };

    if hello.token != environment.authentication_token() {
        let _ = write_error(
            &mut stream,
            "authentication_failed",
            "the CLP authentication token is invalid",
        );
        return;
    }

    let client_hello = ClientHello {
        protocol_version: ProtocolVersion::new(hello.protocol.major, hello.protocol.minor),
        requested_capabilities: capabilities_from_wire(&hello.capabilities),
    };
    let accepted = match manager.lock() {
        Ok(mut manager) => manager.accept(client_hello, ProgramInitContext::default()),
        Err(_) => {
            let _ = write_error(
                &mut stream,
                "host_failure",
                "the CLP session manager is unavailable",
            );
            return;
        }
    };
    let (session_id, init_message) = match accepted {
        Ok(value) => value,
        Err(error) => {
            let _ = write_error(&mut stream, protocol_error_code(&error), &error.to_string());
            return;
        }
    };
    let granted_capabilities = match &init_message {
        HostMessage::Init(init) => init.granted_capabilities,
        _ => unreachable!("a newly accepted CLP session always receives init"),
    };
    let _cleanup = SessionCleanup {
        session_id,
        broker: broker.clone(),
    };

    if let Err(error) = write_host_message(&mut stream, init_message) {
        close_disconnected_session(&manager, session_id);
        eprintln!("Chymeral: could not send CLP init message: {error}");
        return;
    }

    let mut handshake_complete = false;
    loop {
        let deadline = (!handshake_complete).then_some(handshake_deadline);
        let frame = match read_client_frame(&mut stream, &mut frame_reader, deadline, &control) {
            Ok(FrameRead::Message(frame)) => frame,
            Ok(FrameRead::Disconnected) => {
                close_disconnected_session(&manager, session_id);
                return;
            }
            Ok(FrameRead::Shutdown) => {
                close_session(
                    &mut stream,
                    &manager,
                    session_id,
                    SessionCloseReason::HostShutdown,
                );
                return;
            }
            Err(error) => {
                close_protocol_violation(&mut stream, &manager, session_id, &error.to_string());
                return;
            }
        };
        if let WireFrame::Json(payload) = &frame
            && let Ok(WireClientMessage::Command {
                request_id,
                command,
            }) = serde_json::from_slice(payload)
        {
            if !handshake_complete {
                close_protocol_violation(
                    &mut stream,
                    &manager,
                    session_id,
                    "commands require a running session",
                );
                return;
            }
            if request_id.is_empty() {
                let _ = write_error(
                    &mut stream,
                    "invalid_command",
                    "a scene command requires a non-empty request_id",
                );
                continue;
            }
            if !wire_command_is_granted(&command, granted_capabilities) {
                if let Err(error) = discard_scene_command_payload(
                    &command,
                    &mut stream,
                    &mut frame_reader,
                    &control,
                ) {
                    close_protocol_violation(&mut stream, &manager, session_id, &error.to_string());
                    return;
                }
                let _ = write_command_response(
                    &mut stream,
                    &request_id,
                    HostCommandResponse::Error {
                        code: "capability_denied",
                        message:
                            "the command requires a capability that this session was not granted"
                                .to_owned(),
                    },
                );
                continue;
            }
            let command =
                match read_scene_command(command, &mut stream, &mut frame_reader, &control) {
                    Ok(command) => command,
                    Err(error) => {
                        let _ = write_error(&mut stream, "invalid_command", &error.to_string());
                        continue;
                    }
                };
            if !command_is_granted(&command, granted_capabilities) {
                let _ = write_command_response(
                    &mut stream,
                    &request_id,
                    HostCommandResponse::Error {
                        code: "capability_denied",
                        message:
                            "the command requires a capability that this session was not granted"
                                .to_owned(),
                    },
                );
                continue;
            }
            let response =
                dispatch_host_command(&broker, &control, session_id, request_id.clone(), command);
            match response {
                Ok(response) => {
                    if write_command_response(&mut stream, &request_id, response).is_err() {
                        close_disconnected_session(&manager, session_id);
                        return;
                    }
                }
                Err(_) if control.is_shutting_down() => {
                    close_session(
                        &mut stream,
                        &manager,
                        session_id,
                        SessionCloseReason::HostShutdown,
                    );
                    return;
                }
                Err(error) => {
                    let _ = write_command_response(
                        &mut stream,
                        &request_id,
                        HostCommandResponse::Error {
                            code: "host_unavailable",
                            message: error.to_string(),
                        },
                    );
                }
            }
            continue;
        }
        let client_message = match client_message_from_frame(frame) {
            Ok(message) => message,
            Err(error) => {
                close_protocol_violation(&mut stream, &manager, session_id, &error.to_string());
                return;
            }
        };
        let client_started = matches!(client_message, ClientMessage::Started);

        let response = match manager.lock() {
            Ok(mut manager) => manager.receive(session_id, client_message),
            Err(_) => {
                close_protocol_violation(
                    &mut stream,
                    &manager,
                    session_id,
                    "the CLP session manager is unavailable",
                );
                return;
            }
        };
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                close_protocol_violation(&mut stream, &manager, session_id, &error.to_string());
                return;
            }
        };

        if let Some(response) = response {
            let terminal = matches!(response, HostMessage::Closed { .. });
            let write_result = write_host_message(&mut stream, response);
            if terminal {
                remove_terminated_session(&manager, session_id);
                return;
            }
            if let Err(error) = write_result {
                close_disconnected_session(&manager, session_id);
                eprintln!("Chymeral: could not send a CLP lifecycle message: {error}");
                return;
            }
        }

        if client_started {
            handshake_complete = true;
            connection_guard.complete_handshake();
        }
    }
}

struct SessionCleanup {
    session_id: SessionId,
    broker: HostActionBroker,
}

impl Drop for SessionCleanup {
    fn drop(&mut self) {
        self.broker.cleanup(self.session_id);
    }
}

fn close_protocol_violation(
    stream: &mut LocalSocketStream,
    manager: &Arc<Mutex<SessionManager>>,
    session_id: SessionId,
    message: &str,
) {
    let _ = write_error(stream, "protocol_violation", message);
    close_session(
        stream,
        manager,
        session_id,
        SessionCloseReason::ProtocolViolation,
    );
}

fn close_disconnected_session(manager: &Arc<Mutex<SessionManager>>, session_id: SessionId) {
    let mut manager = match manager.lock() {
        Ok(manager) => manager,
        Err(_) => return,
    };
    if manager
        .close(session_id, SessionCloseReason::TransportDisconnected)
        .is_ok()
    {
        let _ = manager.remove_terminated(session_id);
    }
}

fn close_session(
    stream: &mut LocalSocketStream,
    manager: &Arc<Mutex<SessionManager>>,
    session_id: SessionId,
    reason: SessionCloseReason,
) {
    let response = match manager.lock() {
        Ok(mut manager) => manager.close(session_id, reason),
        Err(_) => return,
    };
    if let Ok(response) = response {
        let _ = write_host_message(stream, response);
        remove_terminated_session(manager, session_id);
    }
}

fn remove_terminated_session(manager: &Arc<Mutex<SessionManager>>, session_id: SessionId) {
    if let Ok(mut manager) = manager.lock() {
        let _ = manager.remove_terminated(session_id);
    }
}

fn protocol_error_code(error: &ProtocolError) -> &'static str {
    match error {
        ProtocolError::UnsupportedVersion { .. } => "unsupported_version",
        ProtocolError::SessionIdExhausted | ProtocolError::HandleIdExhausted => "host_capacity",
        ProtocolError::UnknownSession(_)
        | ProtocolError::ActiveSession(_)
        | ProtocolError::InactiveSession(_)
        | ProtocolError::UnexpectedMessage { .. }
        | ProtocolError::HandleNotOwned { .. } => "protocol_violation",
    }
}

fn capabilities_from_wire(capabilities: &[WireCapability]) -> CapabilitySet {
    let mut protocol_capabilities = Vec::with_capacity(capabilities.len());
    for capability in capabilities {
        protocol_capabilities.push(capability.into_protocol());
    }
    CapabilitySet::from_capabilities(&protocol_capabilities)
}

fn client_message_from_frame(frame: WireFrame) -> Result<ClientMessage> {
    let WireFrame::Json(payload) = frame else {
        return Err(anyhow!("expected a JSON CLP message"));
    };
    let message =
        serde_json::from_slice(&payload).context("could not parse the CLP JSON message")?;
    match message {
        WireClientMessage::Hello { .. } => {
            Err(anyhow!("hello may only be sent at connection start"))
        }
        WireClientMessage::InitComplete => Ok(ClientMessage::InitComplete),
        WireClientMessage::Started => Ok(ClientMessage::Started),
        WireClientMessage::Fault { phase, message } => {
            if message.is_empty() {
                return Err(anyhow!("a program fault must include a message"));
            }
            Ok(ClientMessage::Fault(ProgramFault::new(
                phase.into_protocol(),
                message,
            )))
        }
        WireClientMessage::Close => Ok(ClientMessage::Close),
        WireClientMessage::Command { .. } => Err(anyhow!("a scene command was not handled")),
    }
}

fn read_client_message(
    stream: &mut LocalSocketStream,
    frame_reader: &mut FrameReader,
    deadline: Option<Instant>,
    control: &ServerControl,
) -> Result<FrameRead<WireClientMessage>> {
    frame_reader.read_next(stream, deadline, Some(control))
}

fn read_client_frame(
    stream: &mut LocalSocketStream,
    frame_reader: &mut FrameReader,
    deadline: Option<Instant>,
    control: &ServerControl,
) -> Result<FrameRead<WireFrame>> {
    frame_reader.read_next_frame(stream, deadline, Some(control))
}

fn write_host_message(stream: &mut LocalSocketStream, message: HostMessage) -> Result<()> {
    write_frame(stream, &WireHostMessage::from(message))
}

fn write_command_response(
    stream: &mut LocalSocketStream,
    request_id: &str,
    response: HostCommandResponse,
) -> Result<()> {
    match response {
        HostCommandResponse::Success(result) => write_frame(
            stream,
            &WireHostMessage::Result {
                request_id: request_id.to_owned(),
                result: result.into(),
            },
        ),
        HostCommandResponse::Error { code, message } => write_frame(
            stream,
            &WireHostMessage::CommandError {
                request_id: request_id.to_owned(),
                code: code.to_owned(),
                message,
            },
        ),
    }
}

fn dispatch_host_command(
    broker: &HostActionBroker,
    control: &ServerControl,
    session_id: SessionId,
    request_id: String,
    command: SceneCommand,
) -> Result<HostCommandResponse> {
    let (response_sender, response_receiver) = mpsc::sync_channel(1);
    broker.submit(HostCommandRequest {
        session_id,
        request_id,
        command,
        response: response_sender,
    })?;
    loop {
        if control.is_shutting_down() {
            return Err(anyhow!("the CLP host is shutting down"));
        }
        match response_receiver.recv_timeout(LISTENER_POLL_INTERVAL) {
            Ok(response) => return Ok(response),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(anyhow!("the CLP host command response channel closed"));
            }
        }
    }
}

fn read_scene_command(
    command: WireSceneCommand,
    stream: &mut LocalSocketStream,
    frame_reader: &mut FrameReader,
    control: &ServerControl,
) -> Result<SceneCommand> {
    let upload_length = command.upload_length()?;
    let pixels = if let Some(length) = upload_length {
        Some(read_texture_upload(stream, frame_reader, control, length, true)?.unwrap_or_default())
    } else {
        None
    };
    command.into_scene_command(pixels)
}

fn discard_scene_command_payload(
    command: &WireSceneCommand,
    stream: &mut LocalSocketStream,
    frame_reader: &mut FrameReader,
    control: &ServerControl,
) -> Result<()> {
    if let Some(length) = command.upload_length()? {
        let _ = read_texture_upload(stream, frame_reader, control, length, false)?;
    }
    Ok(())
}

fn read_texture_upload(
    stream: &mut LocalSocketStream,
    frame_reader: &mut FrameReader,
    control: &ServerControl,
    length: usize,
    retain_pixels: bool,
) -> Result<Option<Vec<u8>>> {
    let mut received = 0_usize;
    let mut pixels = retain_pixels.then(|| Vec::with_capacity(length));
    while received < length {
        let frame = read_client_frame(stream, frame_reader, None, control)?;
        let FrameRead::Message(WireFrame::Binary(fragment)) = frame else {
            return Err(anyhow!(
                "RGBA8 texture uploads require consecutive binary frames"
            ));
        };
        if fragment.is_empty() {
            return Err(anyhow!("RGBA8 texture upload frames may not be empty"));
        }
        received = received
            .checked_add(fragment.len())
            .context("texture upload byte count overflows this platform")?;
        if received > length {
            return Err(anyhow!(
                "RGBA8 texture upload exceeds its declared byte length"
            ));
        }
        if let Some(pixels) = pixels.as_mut() {
            pixels.extend_from_slice(&fragment);
        }
    }
    Ok(pixels)
}

fn wire_command_is_granted(command: &WireSceneCommand, capabilities: CapabilitySet) -> bool {
    match command {
        WireSceneCommand::UploadTexture { .. } | WireSceneCommand::DestroyTexture { .. } => {
            capabilities.contains(Capability::Resources)
        }
        WireSceneCommand::CreateRoot { content, .. }
        | WireSceneCommand::CreateChild { content, .. }
        | WireSceneCommand::SetContent { content, .. } => {
            capabilities.contains(Capability::Scene)
                && (!content.uses_texture() || capabilities.contains(Capability::Resources))
        }
        WireSceneCommand::UpdateNode { .. }
        | WireSceneCommand::Reparent { .. }
        | WireSceneCommand::RemoveNode { .. } => capabilities.contains(Capability::Scene),
    }
}

fn command_is_granted(command: &SceneCommand, capabilities: CapabilitySet) -> bool {
    match command {
        SceneCommand::UploadTexture { .. } | SceneCommand::DestroyTexture { .. } => {
            capabilities.contains(Capability::Resources)
        }
        SceneCommand::CreateRoot { .. }
        | SceneCommand::CreateChild { .. }
        | SceneCommand::UpdateNode { .. }
        | SceneCommand::SetContent { .. }
        | SceneCommand::Reparent { .. }
        | SceneCommand::RemoveNode { .. } => {
            capabilities.contains(Capability::Scene)
                && (!command_uses_texture(command) || capabilities.contains(Capability::Resources))
        }
    }
}

fn command_uses_texture(command: &SceneCommand) -> bool {
    matches!(
        command,
        SceneCommand::CreateRoot {
            content: ContentInput::Image { .. },
            ..
        } | SceneCommand::CreateChild {
            content: ContentInput::Image { .. },
            ..
        } | SceneCommand::SetContent {
            content: ContentInput::Image { .. },
            ..
        }
    )
}

fn write_error(stream: &mut LocalSocketStream, code: &str, message: &str) -> Result<()> {
    write_frame(
        stream,
        &WireHostMessage::Error {
            code: code.to_owned(),
            message: message.to_owned(),
        },
    )
}

#[derive(Default)]
struct FrameReader {
    bytes: Vec<u8>,
}

enum FrameRead<T> {
    Message(T),
    Disconnected,
    Shutdown,
}

enum WireFrame {
    Json(Vec<u8>),
    Binary(Vec<u8>),
}

impl FrameReader {
    fn read_next<T: DeserializeOwned>(
        &mut self,
        stream: &mut LocalSocketStream,
        deadline: Option<Instant>,
        control: Option<&ServerControl>,
    ) -> Result<FrameRead<T>> {
        loop {
            if control.is_some_and(ServerControl::is_shutting_down) {
                return Ok(FrameRead::Shutdown);
            }
            if let Some(message) = self.take_complete_frame()? {
                let WireFrame::Json(message) = message else {
                    return Err(anyhow!("expected a JSON CLP frame"));
                };
                return serde_json::from_slice(&message)
                    .context("could not parse the CLP JSON message")
                    .map(FrameRead::Message);
            }

            let mut received = [0_u8; 8192];
            match stream.read(&mut received) {
                Ok(0) => {
                    if empty_read_is_disconnected(stream)
                        .context("could not inspect the CLP connection state")?
                    {
                        return Ok(FrameRead::Disconnected);
                    }
                    wait_for_data(deadline)?;
                }
                Ok(count) => self.bytes.extend_from_slice(&received[..count]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if read_disconnected(&error) => return Ok(FrameRead::Disconnected),
                Err(error) if read_would_block(&error) => {
                    wait_for_data(deadline)?;
                }
                Err(error) => return Err(error).context("could not read the CLP frame"),
            }
        }
    }

    fn read_next_frame(
        &mut self,
        stream: &mut LocalSocketStream,
        deadline: Option<Instant>,
        control: Option<&ServerControl>,
    ) -> Result<FrameRead<WireFrame>> {
        loop {
            if control.is_some_and(ServerControl::is_shutting_down) {
                return Ok(FrameRead::Shutdown);
            }
            if let Some(frame) = self.take_complete_frame()? {
                return Ok(FrameRead::Message(frame));
            }
            let mut received = [0_u8; 8192];
            match stream.read(&mut received) {
                Ok(0) => {
                    if empty_read_is_disconnected(stream)
                        .context("could not inspect the CLP connection state")?
                    {
                        return Ok(FrameRead::Disconnected);
                    }
                    wait_for_data(deadline)?;
                }
                Ok(count) => self.bytes.extend_from_slice(&received[..count]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if read_disconnected(&error) => return Ok(FrameRead::Disconnected),
                Err(error) if read_would_block(&error) => wait_for_data(deadline)?,
                Err(error) => return Err(error).context("could not read the CLP frame"),
            }
        }
    }

    fn take_complete_frame(&mut self) -> Result<Option<WireFrame>> {
        if self.bytes.len() < 4 {
            return Ok(None);
        }
        let length = usize::try_from(u32::from_be_bytes(
            self.bytes[..4]
                .try_into()
                .expect("the CLP frame length is present"),
        ))
        .context("the CLP frame length does not fit this platform")?;
        if length > MAX_FRAME_BYTES {
            return Err(anyhow!(
                "the CLP frame is {length} bytes; the limit is {MAX_FRAME_BYTES}"
            ));
        }
        let frame_end = 4_usize
            .checked_add(length)
            .context("the CLP frame length overflows this platform")?;
        if self.bytes.len() < frame_end {
            return Ok(None);
        }
        if length == 0 {
            return Err(anyhow!("CLP frames must include a frame kind"));
        }
        let kind = self.bytes[4];
        let payload = self.bytes[5..frame_end].to_vec();
        self.bytes.drain(..frame_end);
        match kind {
            b'J' => Ok(Some(WireFrame::Json(payload))),
            b'B' => Ok(Some(WireFrame::Binary(payload))),
            _ => Err(anyhow!("CLP frame has an unknown kind")),
        }
    }
}

fn wait_for_data(deadline: Option<Instant>) -> Result<()> {
    let Some(deadline) = deadline else {
        thread::sleep(LISTENER_POLL_INTERVAL);
        return Ok(());
    };
    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
        return Err(anyhow!("the CLP handshake timed out"));
    };
    thread::sleep(remaining.min(LISTENER_POLL_INTERVAL));
    Ok(())
}

#[cfg(target_os = "windows")]
fn empty_read_is_disconnected(stream: &LocalSocketStream) -> io::Result<bool> {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::{
        Foundation::{ERROR_BROKEN_PIPE, ERROR_PIPE_NOT_CONNECTED},
        System::Pipes::PeekNamedPipe,
    };

    let LocalSocketStream::NamedPipe(pipe) = stream;
    let available = unsafe {
        PeekNamedPipe(
            pipe.inner().as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if available != 0 {
        return Ok(false);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(code)
            if code == ERROR_BROKEN_PIPE as i32 || code == ERROR_PIPE_NOT_CONNECTED as i32 =>
        {
            Ok(true)
        }
        _ => Err(error),
    }
}

#[cfg(not(target_os = "windows"))]
fn empty_read_is_disconnected(_: &LocalSocketStream) -> io::Result<bool> {
    Ok(true)
}

fn read_disconnected(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset | io::ErrorKind::UnexpectedEof
    )
}

fn read_would_block(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }

    #[cfg(target_os = "windows")]
    {
        error.raw_os_error() == Some(232)
    }

    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

fn write_frame<T: Serialize>(stream: &mut LocalSocketStream, message: &T) -> Result<()> {
    let payload = serde_json::to_vec(message).context("could not encode the CLP JSON message")?;
    write_raw_frame(stream, b'J', &payload)
}

#[cfg(test)]
fn write_binary_frame(stream: &mut LocalSocketStream, payload: &[u8]) -> Result<()> {
    write_raw_frame(stream, b'B', payload)
}

fn write_raw_frame(stream: &mut LocalSocketStream, kind: u8, payload: &[u8]) -> Result<()> {
    let frame_length = payload
        .len()
        .checked_add(1)
        .context("the CLP frame is too large")?;
    if frame_length > MAX_FRAME_BYTES {
        return Err(anyhow!(
            "the CLP frame is {frame_length} bytes; the limit is {MAX_FRAME_BYTES}"
        ));
    }
    let length = u32::try_from(frame_length).context("the CLP frame is too large")?;
    stream
        .write_all(&length.to_be_bytes())
        .context("could not write the CLP frame length")?;
    stream
        .write_all(&[kind])
        .context("could not write the CLP frame kind")?;
    stream
        .write_all(payload)
        .context("could not write the CLP frame payload")?;
    stream.flush().context("could not flush the CLP frame")
}

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
        command: WireSceneCommand,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum WireSceneCommand {
    CreateRoot {
        #[serde(default)]
        layer: WireSceneLayer,
        properties: WireNodeProperties,
        content: WireNodeContent,
    },
    CreateChild {
        parent: String,
        properties: WireNodeProperties,
        content: WireNodeContent,
    },
    UpdateNode {
        node: String,
        properties: WireNodePropertiesPatch,
    },
    SetContent {
        node: String,
        content: WireNodeContent,
    },
    Reparent {
        node: String,
        parent: Option<String>,
    },
    RemoveNode {
        node: String,
    },
    UploadTexture {
        width: u32,
        height: u32,
    },
    DestroyTexture {
        texture: String,
    },
}

impl WireSceneCommand {
    fn upload_length(&self) -> Result<Option<usize>> {
        let Self::UploadTexture { width, height } = self else {
            return Ok(None);
        };
        let bytes = rgba8_byte_len(*width, *height).context("invalid RGBA8 texture dimensions")?;
        if bytes > MAX_TEXTURE_BYTES {
            return Err(anyhow!(
                "a texture may not exceed {MAX_TEXTURE_BYTES} bytes"
            ));
        }
        Ok(Some(bytes))
    }

    fn into_scene_command(self, pixels: Option<Vec<u8>>) -> Result<SceneCommand> {
        match self {
            Self::CreateRoot {
                layer,
                properties,
                content,
            } => Ok(SceneCommand::CreateRoot {
                layer: layer.into_scene(),
                properties: properties.into_input(),
                content: content.into_input()?,
            }),
            Self::CreateChild {
                parent,
                properties,
                content,
            } => Ok(SceneCommand::CreateChild {
                parent: parse_node_handle(&parent)?,
                properties: properties.into_input(),
                content: content.into_input()?,
            }),
            Self::UpdateNode { node, properties } => Ok(SceneCommand::UpdateNode {
                node: parse_node_handle(&node)?,
                properties: properties.into_patch(),
            }),
            Self::SetContent { node, content } => Ok(SceneCommand::SetContent {
                node: parse_node_handle(&node)?,
                content: content.into_input()?,
            }),
            Self::Reparent { node, parent } => Ok(SceneCommand::Reparent {
                node: parse_node_handle(&node)?,
                parent: parent.as_deref().map(parse_node_handle).transpose()?,
            }),
            Self::RemoveNode { node } => Ok(SceneCommand::RemoveNode {
                node: parse_node_handle(&node)?,
            }),
            Self::UploadTexture { width, height } => Ok(SceneCommand::UploadTexture {
                width,
                height,
                pixels: pixels
                    .ok_or_else(|| anyhow!("an RGBA8 texture upload requires binary pixels"))?,
            }),
            Self::DestroyTexture { texture } => Ok(SceneCommand::DestroyTexture {
                texture: parse_texture_handle(&texture)?,
            }),
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
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireNodeContent {
    Empty,
    Solid { color: [u8; 4] },
    Image { texture: String, opacity: f32 },
}

impl WireNodeContent {
    fn into_input(self) -> Result<ContentInput> {
        match self {
            Self::Empty => Ok(ContentInput::Empty),
            Self::Solid { color } => Ok(ContentInput::Solid { color }),
            Self::Image { texture, opacity } => Ok(ContentInput::Image {
                texture: parse_texture_handle(&texture)?,
                opacity,
            }),
        }
    }

    const fn uses_texture(&self) -> bool {
        matches!(self, Self::Image { .. })
    }
}

fn parse_node_handle(value: &str) -> Result<NodeHandle> {
    parse_handle(value, "node-").map(NodeHandle::new)
}

fn parse_texture_handle(value: &str) -> Result<TextureHandle> {
    parse_handle(value, "texture-").map(TextureHandle::new)
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
    Resources,
    Input,
    Timing,
}

impl WireCapability {
    const fn into_protocol(self) -> Capability {
        match self {
            Self::Scene => Capability::Scene,
            Self::Resources => Capability::Resources,
            Self::Input => Capability::Input,
            Self::Timing => Capability::Timing,
        }
    }
}

impl From<Capability> for WireCapability {
    fn from(capability: Capability) -> Self {
        match capability {
            Capability::Scene => Self::Scene,
            Capability::Resources => Self::Resources,
            Capability::Input => Self::Input,
            Capability::Timing => Self::Timing,
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

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireCommandResult {
    Node { node: String },
    Texture { texture: String },
    Acknowledged,
}

impl From<SceneCommandResult> for WireCommandResult {
    fn from(result: SceneCommandResult) -> Self {
        match result {
            SceneCommandResult::Node(node) => Self::Node {
                node: node.to_string(),
            },
            SceneCommandResult::Texture(texture) => Self::Texture {
                texture: texture.to_string(),
            },
            SceneCommandResult::Acknowledged => Self::Acknowledged,
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
                context: WireInitContext::from(init.context),
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

fn capabilities_to_wire(capabilities: CapabilitySet) -> Vec<WireCapability> {
    [
        Capability::Scene,
        Capability::Resources,
        Capability::Input,
        Capability::Timing,
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
}

impl From<ProgramInitContext> for WireInitContext {
    fn from(context: ProgramInitContext) -> Self {
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
            SessionCloseReason::ProtocolViolation => Self::ProtocolViolation,
            SessionCloseReason::TransportDisconnected => Self::TransportDisconnected,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use interprocess::local_socket::{GenericNamespaced, ToNsName, prelude::LocalSocketStream};
    use serde_json::{Value, json};

    use super::*;

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
                    "resources" => WireCapability::Resources,
                    "input" => WireCapability::Input,
                    "timing" => WireCapability::Timing,
                    _ => unreachable!(),
                })
                .collect(),
        }
    }

    fn send_client(stream: &mut LocalSocketStream, message: WireClientMessage) {
        write_frame(stream, &message).unwrap();
    }

    fn receive_host(stream: &mut LocalSocketStream) -> Value {
        match FrameReader::default()
            .read_next(stream, None, None)
            .unwrap()
        {
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

    fn wait_for_host_action(server: &ClpServer) -> HostAction {
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if let Some(action) = server.drain_host_actions().into_iter().next() {
                return action;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("the CLP host action did not arrive")
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
        assert_eq!(init["protocol"], json!({ "major": 1, "minor": 1 }));
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
    fn forwards_ordered_binary_texture_uploads_to_the_host_thread() {
        let server = ClpServer::start(CapabilitySet::ALL).unwrap();
        let mut client = connect(&server);
        send_client(&mut client, hello(&server, vec!["scene", "resources"]));
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::InitComplete);
        let _ = receive_host(&mut client);
        send_client(&mut client, WireClientMessage::Started);

        send_client(
            &mut client,
            WireClientMessage::Command {
                request_id: "texture-request".to_owned(),
                command: WireSceneCommand::UploadTexture {
                    width: 1,
                    height: 1,
                },
            },
        );
        write_binary_frame(&mut client, &[1, 2, 3, 4]).unwrap();

        let HostAction::Command(request) = wait_for_host_action(&server) else {
            panic!("an active session should forward texture uploads as commands");
        };
        assert_eq!(request.request_id, "texture-request");
        assert_eq!(
            request.command,
            SceneCommand::UploadTexture {
                width: 1,
                height: 1,
                pixels: vec![1, 2, 3, 4],
            }
        );
        request.respond(HostCommandResponse::Success(SceneCommandResult::Texture(
            TextureHandle::new(7),
        )));

        let result = receive_host(&mut client);
        assert_eq!(result["type"], "result");
        assert_eq!(result["request_id"], "texture-request");
        assert_eq!(
            result["result"],
            json!({ "kind": "texture", "texture": "texture-7" })
        );

        send_client(&mut client, WireClientMessage::Close);
        let _ = receive_host(&mut client);
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
                command: WireSceneCommand::UploadTexture {
                    width: 1,
                    height: 1,
                },
            },
        );
        write_binary_frame(&mut client, &[0; 4]).unwrap();

        let error = receive_host(&mut client);
        assert_eq!(error["type"], "command_error");
        assert_eq!(error["request_id"], "denied-upload");
        assert_eq!(error["code"], "capability_denied");
        assert!(server.drain_host_actions().is_empty());
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
        wait_for_session_count(&server, 1);

        server.shutdown();

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
}

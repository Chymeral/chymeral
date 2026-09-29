use std::{
    io::{self, Read, Write},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
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

pub const MLP_ENDPOINT_ENV: &str = "MODITTY_MLP_ENDPOINT";
pub const MLP_TOKEN_ENV: &str = "MODITTY_MLP_TOKEN";
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_PENDING_HANDSHAKES: usize = 16;

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
            (MLP_ENDPOINT_ENV, self.endpoint()),
            (MLP_TOKEN_ENV, self.authentication_token()),
        ]
    }
}

pub struct MlpServer {
    environment: SessionEnvironment,
    listener_thread: Option<thread::JoinHandle<()>>,
    manager: Arc<Mutex<SessionManager>>,
    control: Arc<ServerControl>,
}

impl MlpServer {
    pub fn start(available_capabilities: CapabilitySet) -> Result<Self> {
        let (environment, listener) = bind_listener()?;
        let control = Arc::new(ServerControl::default());
        let manager = Arc::new(Mutex::new(SessionManager::new(available_capabilities)));
        let listener_control = Arc::clone(&control);
        let listener_manager = Arc::clone(&manager);
        let listener_environment = environment.clone();
        let listener_thread = thread::Builder::new()
            .name("moditty-mlp-listener".to_owned())
            .spawn(move || {
                serve_connections(
                    listener,
                    listener_environment,
                    listener_manager,
                    listener_control,
                );
            })
            .context("could not start the MLP listener")?;

        Ok(Self {
            environment,
            listener_thread: Some(listener_thread),
            manager,
            control,
        })
    }

    pub fn environment(&self) -> &SessionEnvironment {
        &self.environment
    }

    pub fn session_count(&self) -> Result<usize> {
        self.manager
            .lock()
            .map(|manager| manager.session_count())
            .map_err(|_| anyhow!("the MLP session manager is unavailable"))
    }

    pub fn shutdown(&mut self) {
        self.control.request_shutdown();
        if let Some(listener_thread) = self.listener_thread.take() {
            let _ = listener_thread.join();
        }
        self.control.wait_for_handlers();
    }

    #[cfg(test)]
    fn pending_handshake_count(&self) -> usize {
        self.control.pending_handshake_count()
    }
}

impl Drop for MlpServer {
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
            .context("could not construct the MLP endpoint name")?;
        match ListenerOptions::new()
            .name(name)
            .nonblocking(ListenerNonblockingMode::Accept)
            .create_sync()
        {
            Ok(listener) => return Ok((environment, listener)),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => continue,
            Err(error) => return Err(error).context("could not bind the local MLP endpoint"),
        }
    }
    Err(anyhow!(
        "could not allocate a unique local MLP endpoint after {ENDPOINT_BIND_ATTEMPTS} attempts"
    ))
}

fn endpoint_name() -> Result<String> {
    Ok(format!("moditty-mlp-{}", random_hex(16)?))
}

fn random_token() -> Result<String> {
    random_hex(32)
}

fn random_hex(byte_count: usize) -> Result<String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut bytes = vec![0_u8; byte_count];
    fill_random(&mut bytes).map_err(|error| {
        anyhow!("could not generate local MLP authentication material: {error}")
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
                if let Err(error) = thread::Builder::new()
                    .name("moditty-mlp-client".to_owned())
                    .spawn(move || {
                        handle_connection(
                            stream,
                            connection_environment,
                            connection_manager,
                            connection_control,
                            connection_guard,
                        );
                    })
                {
                    eprintln!("Moditty: could not serve an MLP client: {error}");
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(LISTENER_POLL_INTERVAL);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                if !control.is_shutting_down() {
                    eprintln!("Moditty: local MLP listener error: {error}");
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
    mut connection_guard: ConnectionGuard,
) {
    if let Err(error) = stream.set_nonblocking(true) {
        eprintln!("Moditty: could not configure nonblocking MLP handshake I/O: {error}");
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
                "the first MLP message must be hello",
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
            "the MLP authentication token is invalid",
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
                "the MLP session manager is unavailable",
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

    if let Err(error) = write_host_message(&mut stream, init_message) {
        close_disconnected_session(&manager, session_id);
        eprintln!("Moditty: could not send MLP init message: {error}");
        return;
    }

    let mut handshake_complete = false;
    loop {
        let deadline = (!handshake_complete).then_some(handshake_deadline);
        let message = match read_client_message(&mut stream, &mut frame_reader, deadline, &control)
        {
            Ok(FrameRead::Message(message)) => message,
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
        let client_message = match client_message_from_wire(message) {
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
                    "the MLP session manager is unavailable",
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
                eprintln!("Moditty: could not send an MLP lifecycle message: {error}");
                return;
            }
        }

        if client_started {
            handshake_complete = true;
            connection_guard.complete_handshake();
        }
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

fn client_message_from_wire(message: WireClientMessage) -> Result<ClientMessage> {
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

fn write_host_message(stream: &mut LocalSocketStream, message: HostMessage) -> Result<()> {
    write_frame(stream, &WireHostMessage::from(message))
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
                return serde_json::from_slice(&message)
                    .context("could not parse the MLP JSON message")
                    .map(FrameRead::Message);
            }

            let mut received = [0_u8; 8192];
            match stream.read(&mut received) {
                Ok(0) => {
                    if empty_read_is_disconnected(stream)
                        .context("could not inspect the MLP connection state")?
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
                Err(error) => return Err(error).context("could not read the MLP frame"),
            }
        }
    }

    fn take_complete_frame(&mut self) -> Result<Option<Vec<u8>>> {
        if self.bytes.len() < 4 {
            return Ok(None);
        }
        let length = usize::try_from(u32::from_be_bytes(
            self.bytes[..4]
                .try_into()
                .expect("the MLP frame length is present"),
        ))
        .context("the MLP frame length does not fit this platform")?;
        if length > MAX_FRAME_BYTES {
            return Err(anyhow!(
                "the MLP frame is {length} bytes; the limit is {MAX_FRAME_BYTES}"
            ));
        }
        let frame_end = 4_usize
            .checked_add(length)
            .context("the MLP frame length overflows this platform")?;
        if self.bytes.len() < frame_end {
            return Ok(None);
        }
        let payload = self.bytes[4..frame_end].to_vec();
        self.bytes.drain(..frame_end);
        Ok(Some(payload))
    }
}

fn wait_for_data(deadline: Option<Instant>) -> Result<()> {
    let Some(deadline) = deadline else {
        thread::sleep(LISTENER_POLL_INTERVAL);
        return Ok(());
    };
    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
        return Err(anyhow!("the MLP handshake timed out"));
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
    let payload = serde_json::to_vec(message).context("could not encode the MLP JSON message")?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(anyhow!(
            "the MLP frame is {} bytes; the limit is {MAX_FRAME_BYTES}",
            payload.len()
        ));
    }
    let length = u32::try_from(payload.len()).context("the MLP frame is too large")?;
    stream
        .write_all(&length.to_be_bytes())
        .context("could not write the MLP frame length")?;
    stream
        .write_all(&payload)
        .context("could not write the MLP frame payload")?;
    stream.flush().context("could not flush the MLP frame")
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

    fn connect(server: &MlpServer) -> LocalSocketStream {
        try_connect(server).unwrap()
    }

    fn try_connect(server: &MlpServer) -> io::Result<LocalSocketStream> {
        let name = server
            .environment()
            .endpoint()
            .to_ns_name::<GenericNamespaced>()
            .unwrap();
        LocalSocketStream::connect(name)
    }

    fn hello(server: &MlpServer, capabilities: Vec<&str>) -> WireClientMessage {
        WireClientMessage::Hello {
            protocol: WireProtocolVersion {
                major: crate::protocol::MLP_PROTOCOL_VERSION.major(),
                minor: crate::protocol::MLP_PROTOCOL_VERSION.minor(),
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
            FrameRead::Disconnected => panic!("the MLP server disconnected before replying"),
            FrameRead::Shutdown => panic!("a standalone MLP client cannot receive host shutdown"),
        }
    }

    fn wait_for_session_count(server: &MlpServer, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if server.session_count().unwrap() == expected {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(server.session_count().unwrap(), expected);
    }

    #[test]
    fn authenticated_client_completes_the_framed_handshake() {
        let server = MlpServer::start(CapabilitySet::from_capabilities(&[
            Capability::Scene,
            Capability::Input,
        ]))
        .unwrap();
        let mut client = connect(&server);

        send_client(&mut client, hello(&server, vec!["scene", "timing"]));
        let init = receive_host(&mut client);
        assert_eq!(init["type"], "init");
        assert_eq!(init["protocol"], json!({ "major": 1, "minor": 0 }));
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
    fn rejects_an_invalid_token_without_creating_a_session() {
        let server = MlpServer::start(CapabilitySet::ALL).unwrap();
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
        let server = MlpServer::start(CapabilitySet::ALL).unwrap();
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
        let server = MlpServer::start(CapabilitySet::ALL).unwrap();
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
        let mut server = MlpServer::start(CapabilitySet::ALL).unwrap();
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
        let server = MlpServer::start(CapabilitySet::ALL).unwrap();
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
        let server = MlpServer::start(CapabilitySet::ALL).unwrap();
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

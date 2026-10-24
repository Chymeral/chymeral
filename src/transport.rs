//! Local Chymeral Link Protocol (CLP) server and session lifecycle.
//!
//! This facade owns endpoint setup, handshake and capability negotiation,
//! session lifecycle, host-action delivery, and notification scheduling.
//! Framing is isolated in the private `framing` module so length-prefix and
//! nonblocking I/O invariants remain shared by JSON and binary CLP messages.
//!
//! Keep host actions, response ordering, bounded notification queues, and
//! session cleanup in this module. They form the host-side concurrency
//! contract observed by the executable, not a public Rust API.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::{self, Read, Write},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

mod framing;
mod notifications;

pub use notifications::ClpNotificationMetrics;

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
use crate::runtime::{
    EngineEvent, FocusEvent, Key, KeyState, Modifiers, NamedKey, PointerButton, PointerEvent,
    SceneEvent, ScrollEvent, ScrollUnit, TerminalEvent, Vec2,
};
use crate::{
    animated_image::MAX_ANIMATED_IMAGE_ENCODED_BYTES,
    program_scene::{
        ClpSceneCommand, ClpSceneCommandResult, ImageHandle, ImageUpload, MAX_PRESENTATION_UPDATES,
        MAX_SCENE_BATCH_BYTES, MAX_TEXTURE_BYTES, NodeChanges, NodeHandle, NodeInput, NodeParent,
        NodePropertiesInput, NodePropertiesPatch, NodeVisualInput, PresentationPacket,
        PresentationPropertiesPatch, PresentationUpdate, SceneBatchCreatedNode,
        SceneBatchNodeChanges, SceneBatchNodeParent, SceneBatchNodeRef, SceneBatchOperation,
        SceneBatchOperationResult, TextNodePropertiesInput,
    },
    resources::rgba8_byte_len,
    scene::{ImageContactShape, NodeId, SceneLayer},
    scene_text::{TextAlign, TextMetrics, TextStyle, TextWeight},
    window_metadata::{MetadataField, WindowMetadataPatch},
};
use framing::{json_frame_bytes, read_would_block, write_frame};
#[cfg(test)]
use framing::{raw_frame_bytes, write_binary_frame, write_raw_frame};
use notifications::{
    DeliveryCategory, HostNotification, NotificationPoll, NotificationQueue, PresentationStatus,
    SessionNotifications, Subscription, SubscriptionError, enqueue_presentation_status,
};

pub const CLP_ENDPOINT_ENV: &str = "CHYMERAL_CLP_ENDPOINT";
pub const CLP_TOKEN_ENV: &str = "CHYMERAL_CLP_TOKEN";
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_PENDING_HANDSHAKES: usize = 16;
pub const MAX_PENDING_HOST_ACTIONS: usize = 64;
pub const MAX_PENDING_HOST_NOTIFICATIONS: usize = 64;
pub const MAX_PRESENTATION_PACKET_BYTES: usize = 64 * 1024;
pub const MAX_PRESENTATION_ID_BYTES: usize = 128;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_GRACE_PERIOD: Duration = Duration::from_millis(250);
const LISTENER_POLL_INTERVAL: Duration = Duration::from_millis(10);
const ENDPOINT_BIND_ATTEMPTS: usize = 8;
const MAX_HOST_NOTIFICATIONS_PER_ITERATION: usize = 4;
const MAX_PENDING_RUNNING_OUTPUT_FRAMES: usize = MAX_HOST_NOTIFICATIONS_PER_ITERATION + 1;
// Windows local named pipes use a 512-byte default output buffer. Capping each
// nonblocking write preserves forward progress for larger CLP frames.
const MAX_RUNNING_FRAME_WRITE_CHUNK_BYTES: usize = 512;

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
    lifecycle_actions: Mutex<Receiver<SessionId>>,
    notifications: Arc<SessionNotifications>,
    presentations: Arc<PresentationMailbox>,
}

/// A frame-presentation packet accepted by the local transport and awaiting a
/// renderer frame. It is intentionally separate from [`HostAction`]: no
/// command response waits for it, and a session retains only its newest packet.
#[derive(Clone, Debug)]
pub struct PresentationSubmission {
    session_id: SessionId,
    packet: PresentationPacket,
    received_at: Instant,
}

impl PresentationSubmission {
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    pub fn packet(&self) -> &PresentationPacket {
        &self.packet
    }

    pub const fn received_at(&self) -> Instant {
        self.received_at
    }
}

#[derive(Default)]
struct PresentationMailbox {
    packets: Mutex<BTreeMap<SessionId, PresentationSubmission>>,
}

impl PresentationMailbox {
    fn submit(
        &self,
        session_id: SessionId,
        packet: PresentationPacket,
    ) -> Option<PresentationSubmission> {
        self.lock_packets().insert(
            session_id,
            PresentationSubmission {
                session_id,
                packet,
                received_at: Instant::now(),
            },
        )
    }

    fn take_all(&self) -> Vec<PresentationSubmission> {
        std::mem::take(&mut *self.lock_packets())
            .into_values()
            .collect()
    }

    fn take(&self, session_id: SessionId) -> Option<PresentationSubmission> {
        self.lock_packets().remove(&session_id)
    }

    fn has_pending(&self) -> bool {
        !self.lock_packets().is_empty()
    }

    fn lock_packets(
        &self,
    ) -> std::sync::MutexGuard<'_, BTreeMap<SessionId, PresentationSubmission>> {
        match self.packets.lock() {
            Ok(packets) => packets,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

pub enum HostAction {
    Command(Box<HostCommandRequest>),
    Cleanup { session_id: SessionId },
    SessionStarted { session_id: SessionId },
}

pub struct HostCommandRequest {
    pub session_id: SessionId,
    pub request_id: String,
    pub command: ClpSceneCommand,
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
        ClpSceneCommand,
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

#[derive(Clone, Debug, PartialEq)]
pub enum HostCommandResponse {
    Success(ClpSceneCommandResult),
    Error { code: &'static str, message: String },
}

#[derive(Clone)]
struct HostActionBroker {
    sender: SyncSender<HostAction>,
    cleanup_sender: mpsc::Sender<SessionId>,
    lifecycle_sender: mpsc::Sender<SessionId>,
    presentations: Arc<PresentationMailbox>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl ClpServer {
    pub fn start(available_capabilities: CapabilitySet) -> Result<Self> {
        Self::start_with_context_and_wake(
            available_capabilities,
            ProgramInitContext::default(),
            || {},
        )
    }

    pub fn start_with_wake(
        available_capabilities: CapabilitySet,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self> {
        Self::start_with_context_and_wake(
            available_capabilities,
            ProgramInitContext::default(),
            wake,
        )
    }

    pub fn start_with_context_and_wake(
        available_capabilities: CapabilitySet,
        init_context: ProgramInitContext,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self> {
        let (environment, listener) = bind_listener()?;
        let control = Arc::new(ServerControl::default());
        let manager = Arc::new(Mutex::new(SessionManager::new(available_capabilities)));
        let (host_action_sender, host_actions) = mpsc::sync_channel(MAX_PENDING_HOST_ACTIONS);
        let (cleanup_sender, cleanup_actions) = mpsc::channel();
        let (lifecycle_sender, lifecycle_actions) = mpsc::channel();
        let notifications = Arc::new(SessionNotifications::default());
        let presentations = Arc::new(PresentationMailbox::default());
        let broker = HostActionBroker {
            sender: host_action_sender,
            cleanup_sender,
            lifecycle_sender,
            presentations: Arc::clone(&presentations),
            wake: Arc::new(wake),
        };
        let listener_control = Arc::clone(&control);
        let listener_manager = Arc::clone(&manager);
        let listener_environment = environment.clone();
        let listener_init_context = init_context;
        let listener_broker = broker.clone();
        let listener_notifications = Arc::clone(&notifications);
        let listener_thread = thread::Builder::new()
            .name("chymeral-clp-listener".to_owned())
            .spawn(move || {
                serve_connections(
                    listener,
                    listener_environment,
                    listener_init_context,
                    listener_manager,
                    listener_control,
                    listener_broker,
                    listener_notifications,
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
            lifecycle_actions: Mutex::new(lifecycle_actions),
            notifications,
            presentations,
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

    pub fn notification_metrics(&self) -> ClpNotificationMetrics {
        self.notifications.metrics()
    }

    /// Returns the newest accepted presentation packet for every session.
    ///
    /// The window loop consumes this only immediately before rendering. This
    /// keeps presentation traffic out of the ordered command queue while
    /// retaining deterministic per-session ownership checks at commit time.
    pub fn drain_presentation_packets(&self) -> Vec<PresentationSubmission> {
        self.presentations.take_all()
    }

    pub fn has_pending_presentation_packets(&self) -> bool {
        self.presentations.has_pending()
    }

    /// Discards a packet that has not reached the renderer because a later
    /// reliable mutation for the same session superseded it.
    pub fn discard_pending_presentation(
        &self,
        session_id: SessionId,
    ) -> Option<PresentationSubmission> {
        self.presentations.take(session_id)
    }

    pub fn report_presentation_presented(&self, session_id: SessionId, presentation_id: String) {
        self.report_presentation_status(session_id, presentation_id, PresentationStatus::Presented);
    }

    pub fn report_presentation_rejected(
        &self,
        session_id: SessionId,
        presentation_id: String,
        code: impl Into<String>,
        message: impl Into<String>,
    ) {
        self.report_presentation_status(
            session_id,
            presentation_id,
            PresentationStatus::Rejected {
                code: code.into(),
                message: message.into(),
            },
        );
    }

    pub fn report_presentation_superseded(&self, session_id: SessionId, presentation_id: String) {
        self.report_presentation_status(
            session_id,
            presentation_id,
            PresentationStatus::Superseded,
        );
    }

    fn report_presentation_status(
        &self,
        session_id: SessionId,
        presentation_id: String,
        status: PresentationStatus,
    ) {
        enqueue_presentation_status(&self.notifications, session_id, presentation_id, status);
    }

    pub fn shutdown(&mut self) {
        self.control.request_shutdown();
        if let Some(listener_thread) = self.listener_thread.take() {
            let _ = listener_thread.join();
        }
        self.control.wait_for_handlers();
    }

    pub fn activate_session(
        &self,
        session_id: SessionId,
        physical_width: u32,
        physical_height: u32,
        scale_factor: f64,
    ) {
        let _ = self.notifications.activate(
            session_id,
            resize_notification(physical_width, physical_height, scale_factor),
        );
    }

    pub fn broadcast_event(&self, event: &EngineEvent) {
        let Some(event) = WireEvent::from_engine_event(event) else {
            return;
        };
        match event {
            WireEvent::Focus { .. } => self
                .notifications
                .broadcast_matching(HostNotification::Event(event), |session| {
                    session.capabilities.contains(Capability::Input)
                }),
            WireEvent::Terminal { .. } => {
                self.notifications
                    .broadcast_matching(HostNotification::Event(event), |session| {
                        session.capabilities.contains(Capability::Input)
                            && session.subscriptions.contains(&Subscription::Terminal)
                    })
            }
            WireEvent::Custom { .. } => {
                self.notifications
                    .broadcast_matching(HostNotification::Event(event), |session| {
                        session.capabilities.contains(Capability::Input)
                            && session.subscriptions.contains(&Subscription::RawInput)
                    })
            }
            WireEvent::Key { .. }
            | WireEvent::PointerMove { .. }
            | WireEvent::PointerPress { .. }
            | WireEvent::PointerRelease { .. }
            | WireEvent::Scroll { .. }
            | WireEvent::Scene { .. } => {}
        }
    }

    pub fn send_keyboard_event(&self, session_id: SessionId, event: &crate::runtime::KeyEvent) {
        let event = WireEvent::from_engine_event(&EngineEvent::Key(event.clone()))
            .expect("key events are serializable CLP events");
        self.notifications
            .send_matching(session_id, HostNotification::Event(event), |session| {
                session.capabilities.contains(Capability::Input)
            });
    }

    pub fn send_raw_input_event(&self, session_id: SessionId, event: &EngineEvent) {
        let Some(event) = WireEvent::from_engine_event(event) else {
            return;
        };
        if !matches!(
            event,
            WireEvent::PointerMove { .. }
                | WireEvent::PointerPress { .. }
                | WireEvent::PointerRelease { .. }
                | WireEvent::Scroll { .. }
        ) {
            return;
        }
        self.notifications
            .send_matching(session_id, HostNotification::Event(event), |session| {
                session.capabilities.contains(Capability::Input)
                    && session.subscriptions.contains(&Subscription::RawInput)
            });
    }

    pub fn broadcast_scene_event(
        &self,
        event: &SceneEvent,
        node_handle_for: impl Fn(SessionId, NodeId) -> Option<NodeHandle>,
    ) {
        self.notifications.broadcast_scene(event, node_handle_for);
    }

    pub fn broadcast_tick(&self, dt: Duration) {
        self.notifications.broadcast_matching(
            HostNotification::Tick {
                dt_ns: dt.as_nanos().to_string(),
            },
            |session| {
                session.capabilities.contains(Capability::Timing)
                    && session.subscriptions.contains(&Subscription::Timing)
            },
        );
    }

    pub fn broadcast_frame(&self, dt: Duration) {
        self.notifications.broadcast_matching(
            HostNotification::Frame {
                dt_ns: dt.as_nanos().to_string(),
            },
            |session| {
                session.capabilities.contains(Capability::Timing)
                    && session.subscriptions.contains(&Subscription::Timing)
            },
        );
    }

    pub fn broadcast_resize(&self, physical_width: u32, physical_height: u32, scale_factor: f64) {
        self.notifications.broadcast_matching(
            resize_notification(physical_width, physical_height, scale_factor),
            |session| session.capabilities.contains(Capability::Input),
        );
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
        if let Ok(receiver) = self.lifecycle_actions.lock() {
            actions.extend(
                std::iter::from_fn(|| receiver.try_recv().ok())
                    .map(|session_id| HostAction::SessionStarted { session_id }),
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
        match self.sender.try_send(HostAction::Command(Box::new(request))) {
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

    fn submit_presentation(
        &self,
        session_id: SessionId,
        packet: PresentationPacket,
    ) -> Option<PresentationSubmission> {
        let replaced = self.presentations.submit(session_id, packet);
        (self.wake)();
        replaced
    }

    fn cleanup(&self, session_id: SessionId) {
        self.presentations.take(session_id);
        if self.cleanup_sender.send(session_id).is_ok() {
            (self.wake)();
        }
    }

    fn started(&self, session_id: SessionId) {
        if self.lifecycle_sender.send(session_id).is_ok() {
            (self.wake)();
        }
    }
}

fn resize_notification(
    physical_width: u32,
    physical_height: u32,
    scale_factor: f64,
) -> HostNotification {
    let scale_factor = scale_factor.max(f64::MIN_POSITIVE);
    HostNotification::Resize {
        physical_width,
        physical_height,
        logical_width: f64::from(physical_width) / scale_factor,
        logical_height: f64::from(physical_height) / scale_factor,
        scale_factor,
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
    init_context: ProgramInitContext,
    manager: Arc<Mutex<SessionManager>>,
    control: Arc<ServerControl>,
    broker: HostActionBroker,
    notifications: Arc<SessionNotifications>,
) {
    while !control.is_shutting_down() {
        match listener.accept() {
            Ok(stream) => {
                let Some(connection_guard) = control.try_register_connection() else {
                    drop(stream);
                    continue;
                };
                let connection = ConnectionContext {
                    environment: environment.clone(),
                    init_context: init_context.clone(),
                    manager: Arc::clone(&manager),
                    control: Arc::clone(&control),
                    broker: broker.clone(),
                    notifications: Arc::clone(&notifications),
                };
                if let Err(error) = thread::Builder::new()
                    .name("chymeral-clp-client".to_owned())
                    .spawn(move || {
                        handle_connection(stream, connection, connection_guard);
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

struct ConnectionContext {
    environment: SessionEnvironment,
    init_context: ProgramInitContext,
    manager: Arc<Mutex<SessionManager>>,
    control: Arc<ServerControl>,
    broker: HostActionBroker,
    notifications: Arc<SessionNotifications>,
}

fn handle_connection(
    mut stream: LocalSocketStream,
    connection: ConnectionContext,
    mut connection_guard: ConnectionGuard,
) {
    let ConnectionContext {
        environment,
        init_context,
        manager,
        control,
        broker,
        notifications,
    } = connection;
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
        Ok(mut manager) => manager.accept(client_hello, init_context),
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
        notifications: Arc::clone(&notifications),
    };

    if let Err(error) = write_host_message(&mut stream, init_message) {
        close_disconnected_session(&manager, session_id);
        eprintln!("Chymeral: could not send CLP init message: {error}");
        return;
    }

    loop {
        let frame = match read_client_frame(
            &mut stream,
            &mut frame_reader,
            Some(handshake_deadline),
            &control,
        ) {
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
                request_id: _,
                command: _,
            }) = serde_json::from_slice(payload)
        {
            close_protocol_violation(
                &mut stream,
                &manager,
                session_id,
                "commands require a running session",
            );
            return;
        }
        if let WireFrame::Json(payload) = &frame
            && let Ok(WireClientMessage::PresentScene { .. }) = serde_json::from_slice(payload)
        {
            close_protocol_violation(
                &mut stream,
                &manager,
                session_id,
                "presentation packets require a running session",
            );
            return;
        }
        if let WireFrame::Json(payload) = &frame
            && let Ok(WireClientMessage::Subscribe { .. }) = serde_json::from_slice(payload)
        {
            close_protocol_violation(
                &mut stream,
                &manager,
                session_id,
                "subscriptions require a running session",
            );
            return;
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
            connection_guard.complete_handshake();
            let notification_queue = notifications.register(session_id, granted_capabilities);
            broker.started(session_id);
            run_running_session(
                &mut stream,
                &mut frame_reader,
                &manager,
                &control,
                &broker,
                &notifications,
                RunningSession {
                    session_id,
                    granted_capabilities,
                    notification_queue,
                },
            );
            return;
        }
    }
}

struct RunningSession {
    session_id: SessionId,
    granted_capabilities: CapabilitySet,
    notification_queue: Arc<NotificationQueue>,
}

#[derive(Default)]
struct RunningFrameWriter {
    frames: VecDeque<PendingHostFrame>,
}

struct PendingHostFrame {
    bytes: Vec<u8>,
    written: usize,
}

impl RunningFrameWriter {
    fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    fn is_full(&self) -> bool {
        self.frames.len() >= MAX_PENDING_RUNNING_OUTPUT_FRAMES
    }

    fn can_queue_notification(&self) -> bool {
        self.frames.len() < MAX_HOST_NOTIFICATIONS_PER_ITERATION
    }

    fn queue(&mut self, message: &impl Serialize) -> Result<()> {
        if self.is_full() {
            return Err(anyhow!("the CLP running output queue is full"));
        }
        self.frames.push_back(PendingHostFrame {
            bytes: json_frame_bytes(message)?,
            written: 0,
        });
        Ok(())
    }

    fn flush(&mut self, stream: &mut impl Write) -> io::Result<()> {
        loop {
            let complete = {
                let Some(frame) = self.frames.front_mut() else {
                    return Ok(());
                };
                while frame.written < frame.bytes.len() {
                    let remaining = &frame.bytes[frame.written..];
                    let chunk_len = remaining.len().min(MAX_RUNNING_FRAME_WRITE_CHUNK_BYTES);
                    match stream.write(&remaining[..chunk_len]) {
                        Ok(0) => return Ok(()),
                        Ok(count) => frame.written += count,
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(error) if read_would_block(&error) => return Ok(()),
                        Err(error) => return Err(error),
                    }
                }
                true
            };
            if complete {
                let _ = self.frames.pop_front();
            }
        }
    }
}

fn run_running_session(
    stream: &mut LocalSocketStream,
    frame_reader: &mut FrameReader,
    manager: &Arc<Mutex<SessionManager>>,
    control: &Arc<ServerControl>,
    broker: &HostActionBroker,
    notifications: &SessionNotifications,
    session: RunningSession,
) {
    let RunningSession {
        session_id,
        granted_capabilities,
        notification_queue,
    } = session;
    let mut writer = RunningFrameWriter::default();
    let mut terminal_response_queued = false;
    loop {
        if control.is_shutting_down() {
            if writer.is_empty() {
                close_for_host_shutdown(stream, frame_reader, manager, session_id);
            } else {
                terminate_session(manager, session_id, SessionCloseReason::HostShutdown);
            }
            return;
        }

        if let Err(error) = writer.flush(stream) {
            eprintln!("Chymeral: could not flush CLP host output for {session_id}: {error:#}");
            close_disconnected_session(manager, session_id);
            return;
        }
        if terminal_response_queued && writer.is_empty() {
            remove_terminated_session(manager, session_id);
            return;
        }

        if !terminal_response_queued {
            let mut queued_notifications = 0;
            while queued_notifications < MAX_HOST_NOTIFICATIONS_PER_ITERATION
                && writer.can_queue_notification()
            {
                let notification = match notification_queue.try_pop() {
                    NotificationPoll::Notification(notification) => notification,
                    NotificationPoll::Overloaded => {
                        if writer.is_empty() {
                            close_session(
                                stream,
                                manager,
                                session_id,
                                SessionCloseReason::HostOverloaded,
                            );
                        } else {
                            terminate_session(
                                manager,
                                session_id,
                                SessionCloseReason::HostOverloaded,
                            );
                        }
                        return;
                    }
                    NotificationPoll::Empty => break,
                };
                if let Err(error) = writer.queue(&WireHostMessage::from_notification(
                    session_id,
                    notification,
                )) {
                    eprintln!(
                        "Chymeral: could not queue a CLP host notification for {session_id}: {error:#}"
                    );
                    close_disconnected_session(manager, session_id);
                    return;
                }
                queued_notifications += 1;
            }

            if let Err(error) = writer.flush(stream) {
                eprintln!("Chymeral: could not flush CLP host output for {session_id}: {error:#}");
                close_disconnected_session(manager, session_id);
                return;
            }
        }

        if writer.is_full() {
            thread::sleep(LISTENER_POLL_INTERVAL);
            continue;
        }

        let frame = match frame_reader.try_read_next_frame(stream, Some(control)) {
            Ok(Some(FrameRead::Message(frame))) => frame,
            Ok(Some(FrameRead::Disconnected)) => {
                close_disconnected_session(manager, session_id);
                return;
            }
            Ok(Some(FrameRead::Shutdown)) => {
                close_for_host_shutdown(stream, frame_reader, manager, session_id);
                return;
            }
            Ok(None) => {
                thread::sleep(LISTENER_POLL_INTERVAL);
                continue;
            }
            Err(error) => {
                close_protocol_violation(stream, manager, session_id, &error.to_string());
                return;
            }
        };

        if let WireFrame::Json(payload) = &frame
            && let Ok(WireClientMessage::Command {
                request_id,
                command,
            }) = serde_json::from_slice(payload)
        {
            if request_id.is_empty() {
                let _ = writer.queue(&WireHostMessage::Error {
                    code: "invalid_command".to_owned(),
                    message: "a scene command requires a non-empty request_id".to_owned(),
                });
                continue;
            }
            if command.is_scene_batch() && payload.len() > MAX_SCENE_BATCH_BYTES {
                let _ = writer.queue(&command_response_message(
                    &request_id,
                    HostCommandResponse::Error {
                        code: "resource_limit",
                        message: format!(
                            "a scene batch may not exceed {MAX_SCENE_BATCH_BYTES} bytes"
                        ),
                    },
                ));
                continue;
            }
            if !wire_command_is_granted(&command, granted_capabilities) {
                if let Err(error) =
                    discard_scene_command_payload(&command, stream, frame_reader, control)
                {
                    close_protocol_violation(stream, manager, session_id, &error.to_string());
                    return;
                }
                let _ = writer.queue(&command_response_message(
                    &request_id,
                    HostCommandResponse::Error {
                        code: "capability_denied",
                        message:
                            "the command requires a capability that this session was not granted"
                                .to_owned(),
                    },
                ));
                continue;
            }
            let command = match read_scene_command(*command, stream, frame_reader, control) {
                Ok(command) => command,
                Err(error) => {
                    let _ = writer.queue(&WireHostMessage::Error {
                        code: "invalid_command".to_owned(),
                        message: error.to_string(),
                    });
                    continue;
                }
            };
            let response =
                dispatch_host_command(broker, control, session_id, request_id.clone(), command);
            match response {
                Ok(response) => {
                    if writer
                        .queue(&command_response_message(&request_id, response))
                        .is_err()
                    {
                        close_disconnected_session(manager, session_id);
                        return;
                    }
                }
                Err(_) if control.is_shutting_down() => {
                    close_for_host_shutdown(stream, frame_reader, manager, session_id);
                    return;
                }
                Err(error) => {
                    let _ = writer.queue(&command_response_message(
                        &request_id,
                        HostCommandResponse::Error {
                            code: "host_unavailable",
                            message: error.to_string(),
                        },
                    ));
                }
            }
            continue;
        }

        if let WireFrame::Json(payload) = &frame
            && let Ok(WireClientMessage::PresentScene {
                presentation_id,
                updates,
            }) = serde_json::from_slice(payload)
        {
            if presentation_id.is_empty() || presentation_id.len() > MAX_PRESENTATION_ID_BYTES {
                let _ = writer.queue(&WireHostMessage::Error {
                    code: "invalid_presentation".to_owned(),
                    message: format!(
                        "a presentation_id must contain between 1 and {MAX_PRESENTATION_ID_BYTES} bytes"
                    ),
                });
                continue;
            }
            if !wire_presentation_is_granted(granted_capabilities) {
                enqueue_presentation_status(
                    notifications,
                    session_id,
                    presentation_id,
                    PresentationStatus::Rejected {
                        code: "capability_denied".to_owned(),
                        message: "presentation requires the scene and presentation capabilities"
                            .to_owned(),
                    },
                );
                continue;
            }
            if payload.len() > MAX_PRESENTATION_PACKET_BYTES {
                enqueue_presentation_status(
                    notifications,
                    session_id,
                    presentation_id,
                    PresentationStatus::Rejected {
                        code: "presentation_too_large".to_owned(),
                        message: format!(
                            "a presentation packet may not exceed {MAX_PRESENTATION_PACKET_BYTES} bytes"
                        ),
                    },
                );
                continue;
            }
            match presentation_packet_from_wire(presentation_id.clone(), updates) {
                Ok(packet) => {
                    if let Some(replaced) = broker.submit_presentation(session_id, packet) {
                        enqueue_presentation_status(
                            notifications,
                            session_id,
                            replaced.packet.presentation_id,
                            PresentationStatus::Superseded,
                        );
                    }
                }
                Err(error) => enqueue_presentation_status(
                    notifications,
                    session_id,
                    presentation_id,
                    PresentationStatus::Rejected {
                        code: "invalid_presentation".to_owned(),
                        message: error.to_string(),
                    },
                ),
            }
            continue;
        }

        if let WireFrame::Json(payload) = &frame
            && let Ok(WireClientMessage::Subscribe { subscriptions }) =
                serde_json::from_slice(payload)
        {
            let subscriptions = subscriptions
                .into_iter()
                .map(WireSubscription::into_subscription)
                .collect();
            match notifications.set_subscriptions(session_id, subscriptions) {
                Ok(subscriptions) => {
                    let message = subscribed_message(session_id, subscriptions);
                    if writer.queue(&message).is_err() {
                        close_disconnected_session(manager, session_id);
                        return;
                    }
                }
                Err(SubscriptionError::CapabilityDenied) => {
                    let _ = writer.queue(&WireHostMessage::Error {
                        code: "capability_denied".to_owned(),
                        message:
                            "one or more subscriptions require a capability that this session was not granted"
                                .to_owned(),
                    });
                }
                Err(SubscriptionError::SessionUnavailable) => {
                    close_disconnected_session(manager, session_id);
                    return;
                }
            }
            continue;
        }

        let client_message = match client_message_from_frame(frame) {
            Ok(message) => message,
            Err(error) => {
                if writer.is_empty() {
                    close_protocol_violation(stream, manager, session_id, &error.to_string());
                } else {
                    terminate_session(manager, session_id, SessionCloseReason::ProtocolViolation);
                }
                return;
            }
        };
        let response = match manager.lock() {
            Ok(mut manager) => manager.receive(session_id, client_message),
            Err(_) => {
                close_protocol_violation(
                    stream,
                    manager,
                    session_id,
                    "the CLP session manager is unavailable",
                );
                return;
            }
        };
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                if writer.is_empty() {
                    close_protocol_violation(stream, manager, session_id, &error.to_string());
                } else {
                    terminate_session(manager, session_id, SessionCloseReason::ProtocolViolation);
                }
                return;
            }
        };
        if let Some(response) = response {
            let terminal = matches!(response, HostMessage::Closed { .. });
            if writer.queue(&WireHostMessage::from(response)).is_err() {
                close_disconnected_session(manager, session_id);
                return;
            }
            terminal_response_queued = terminal;
        }
    }
}

fn close_for_host_shutdown(
    stream: &mut LocalSocketStream,
    frame_reader: &mut FrameReader,
    manager: &Arc<Mutex<SessionManager>>,
    session_id: SessionId,
) {
    let _ = write_host_closing(stream, session_id, SessionCloseReason::HostShutdown);
    let deadline = Instant::now() + CLOSE_GRACE_PERIOD;
    while let Ok(FrameRead::Message(frame)) =
        frame_reader.read_next_frame(stream, Some(deadline), None)
    {
        if matches!(client_message_from_frame(frame), Ok(ClientMessage::Close)) {
            break;
        }
    }
    close_session(
        stream,
        manager,
        session_id,
        SessionCloseReason::HostShutdown,
    );
}

struct SessionCleanup {
    session_id: SessionId,
    broker: HostActionBroker,
    notifications: Arc<SessionNotifications>,
}

impl Drop for SessionCleanup {
    fn drop(&mut self) {
        self.notifications.unregister(self.session_id);
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
    terminate_session(
        manager,
        session_id,
        SessionCloseReason::TransportDisconnected,
    );
}

fn terminate_session(
    manager: &Arc<Mutex<SessionManager>>,
    session_id: SessionId,
    reason: SessionCloseReason,
) {
    let mut manager = match manager.lock() {
        Ok(manager) => manager,
        Err(_) => return,
    };
    if manager.close(session_id, reason).is_ok() {
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
        WireClientMessage::PresentScene { .. } => {
            Err(anyhow!("a presentation packet was not handled"))
        }
        WireClientMessage::Subscribe { .. } => {
            Err(anyhow!("a subscription update was not handled"))
        }
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

fn subscribed_message(
    session_id: SessionId,
    subscriptions: BTreeSet<Subscription>,
) -> WireHostMessage {
    WireHostMessage::Subscribed {
        session_id: session_id.to_string(),
        subscriptions: subscriptions
            .into_iter()
            .map(WireSubscription::from)
            .collect(),
    }
}

fn write_host_closing(
    stream: &mut LocalSocketStream,
    session_id: SessionId,
    reason: SessionCloseReason,
) -> Result<()> {
    write_frame(
        stream,
        &WireHostMessage::Closing {
            session_id: session_id.to_string(),
            reason: reason.into(),
        },
    )
}

fn command_response_message(request_id: &str, response: HostCommandResponse) -> WireHostMessage {
    match response {
        HostCommandResponse::Success(result) => WireHostMessage::Result {
            request_id: request_id.to_owned(),
            result: result.into(),
        },
        HostCommandResponse::Error { code, message } => WireHostMessage::CommandError {
            request_id: request_id.to_owned(),
            code: code.to_owned(),
            message,
        },
    }
}

fn dispatch_host_command(
    broker: &HostActionBroker,
    control: &ServerControl,
    session_id: SessionId,
    request_id: String,
    command: ClpSceneCommand,
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
) -> Result<ClpSceneCommand> {
    let upload_length = command.upload_length()?;
    let payload = if let Some(length) = upload_length {
        Some(read_binary_upload(stream, frame_reader, control, length, true)?.unwrap_or_default())
    } else {
        None
    };
    command.into_clp_scene_command(payload)
}

fn discard_scene_command_payload(
    command: &WireSceneCommand,
    stream: &mut LocalSocketStream,
    frame_reader: &mut FrameReader,
    control: &ServerControl,
) -> Result<()> {
    if let Some(length) = command.upload_length()? {
        let _ = read_binary_upload(stream, frame_reader, control, length, false)?;
    }
    Ok(())
}

fn read_binary_upload(
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
                "binary resource uploads require consecutive binary frames"
            ));
        };
        if fragment.is_empty() {
            return Err(anyhow!("binary resource upload frames may not be empty"));
        }
        received = received
            .checked_add(fragment.len())
            .context("binary resource upload byte count overflows this platform")?;
        if received > length {
            return Err(anyhow!(
                "binary resource upload exceeds its declared byte length"
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
        WireSceneCommand::SceneBatch { operations } => {
            capabilities.contains(Capability::Scene)
                && capabilities.contains(Capability::SceneBatch)
                && operations.iter().all(|operation| {
                    (!operation.uses_image() || capabilities.contains(Capability::Images))
                        && (!operation.uses_text() || capabilities.contains(Capability::Text))
                        && (!operation.uses_overlap() || capabilities.contains(Capability::Overlap))
                        && (!operation.uses_external_drop()
                            || capabilities.contains(Capability::ExternalDrop))
                })
        }
        WireSceneCommand::UploadImage { .. } | WireSceneCommand::RemoveImage { .. } => {
            capabilities.contains(Capability::Images)
        }
        WireSceneCommand::IsOverlapping { .. } | WireSceneCommand::GetOverlaps { .. } => {
            capabilities.contains(Capability::Scene) && capabilities.contains(Capability::Overlap)
        }
        WireSceneCommand::SetWindowMetadata { .. } => {
            capabilities.contains(Capability::WindowMetadata)
        }
        WireSceneCommand::RemoveNode { .. } => capabilities.contains(Capability::Scene),
        WireSceneCommand::CreateNode { node, .. } => {
            capabilities.contains(Capability::Scene)
                && (!node.uses_image() || capabilities.contains(Capability::Images))
                && (!node.uses_text() || capabilities.contains(Capability::Text))
                && (!node.uses_overlap() || capabilities.contains(Capability::Overlap))
                && (!node.uses_external_drop() || capabilities.contains(Capability::ExternalDrop))
        }
        WireSceneCommand::SetNode { changes, .. } => {
            capabilities.contains(Capability::Scene)
                && (!changes.uses_image() || capabilities.contains(Capability::Images))
                && (!changes.uses_text() || capabilities.contains(Capability::Text))
                && (!changes.uses_overlap() || capabilities.contains(Capability::Overlap))
                && (!changes.uses_external_drop()
                    || capabilities.contains(Capability::ExternalDrop))
        }
    }
}

fn wire_presentation_is_granted(capabilities: CapabilitySet) -> bool {
    capabilities.contains(Capability::Scene) && capabilities.contains(Capability::Presentation)
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

    fn try_read_next_frame(
        &mut self,
        stream: &mut LocalSocketStream,
        control: Option<&ServerControl>,
    ) -> Result<Option<FrameRead<WireFrame>>> {
        if control.is_some_and(ServerControl::is_shutting_down) {
            return Ok(Some(FrameRead::Shutdown));
        }
        if let Some(frame) = self.take_complete_frame()? {
            return Ok(Some(FrameRead::Message(frame)));
        }

        let mut received = [0_u8; 8192];
        loop {
            match stream.read(&mut received) {
                Ok(0) => {
                    if empty_read_is_disconnected(stream)
                        .context("could not inspect the CLP connection state")?
                    {
                        return Ok(Some(FrameRead::Disconnected));
                    }
                    return Ok(None);
                }
                Ok(count) => {
                    self.bytes.extend_from_slice(&received[..count]);
                    if let Some(frame) = self.take_complete_frame()? {
                        return Ok(Some(FrameRead::Message(frame)));
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if read_disconnected(&error) => {
                    return Ok(Some(FrameRead::Disconnected));
                }
                Err(error) if read_would_block(&error) => return Ok(None),
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
    // SAFETY: `pipe` owns a live named-pipe handle for the duration of this
    // call. A zero-length peek permits null optional output pointers and does
    // not read or write a caller-provided buffer.
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

// The wire schema remains in the transport module's privacy boundary so the
// test corpus can exercise private malformed-frame cases without widening its
// visibility. It is kept in a separate source file for navigation.
include!("transport/wire.rs");

#[cfg(test)]
mod tests {
    include!("transport/tests.rs");
}

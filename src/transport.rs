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
        ClpSceneCommand, ClpSceneCommandResult, ImageHandle, ImageUpload, MAX_TEXTURE_BYTES,
        NodeChanges, NodeHandle, NodeInput, NodeParent, NodePropertiesInput, NodePropertiesPatch,
        NodeVisualInput, TextNodePropertiesInput,
    },
    resources::rgba8_byte_len,
    scene::{ImageContactShape, NodeId, SceneLayer},
    scene_text::{TextAlign, TextMetrics, TextStyle, TextWeight},
};

pub const CLP_ENDPOINT_ENV: &str = "CHYMERAL_CLP_ENDPOINT";
pub const CLP_TOKEN_ENV: &str = "CHYMERAL_CLP_TOKEN";
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_PENDING_HANDSHAKES: usize = 16;
pub const MAX_PENDING_HOST_ACTIONS: usize = 64;
pub const MAX_PENDING_HOST_NOTIFICATIONS: usize = 64;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_GRACE_PERIOD: Duration = Duration::from_millis(250);
const LISTENER_POLL_INTERVAL: Duration = Duration::from_millis(10);
const ENDPOINT_BIND_ATTEMPTS: usize = 8;
const MAX_HOST_NOTIFICATIONS_PER_ITERATION: usize = 4;
const MAX_PENDING_RUNNING_OUTPUT_FRAMES: usize = MAX_HOST_NOTIFICATIONS_PER_ITERATION + 1;
const WRITE_RETRY_INTERVAL: Duration = Duration::from_millis(1);
const WRITE_STALL_TIMEOUT: Duration = Duration::from_secs(1);

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
    wake: Arc<dyn Fn() + Send + Sync>,
}

#[derive(Clone)]
enum HostNotification {
    Event(WireEvent),
    Tick {
        dt_ns: String,
    },
    Frame {
        dt_ns: String,
    },
    Resize {
        physical_width: u32,
        physical_height: u32,
        logical_width: f64,
        logical_height: f64,
        scale_factor: f64,
    },
    DeliveryGap {
        categories: BTreeSet<DeliveryCategory>,
    },
}

impl HostNotification {
    const fn delivery_category(&self) -> Option<DeliveryCategory> {
        match self {
            Self::Event(event) => event.delivery_category(),
            Self::Tick { .. } | Self::Frame { .. } => Some(DeliveryCategory::Timing),
            Self::Resize { .. } | Self::DeliveryGap { .. } => None,
        }
    }

    const fn coalescing_class(&self) -> Option<NotificationCoalescingClass> {
        match self {
            Self::Tick { .. } => Some(NotificationCoalescingClass::Tick),
            Self::Frame { .. } => Some(NotificationCoalescingClass::Frame),
            Self::Event(WireEvent::PointerMove { .. }) => {
                Some(NotificationCoalescingClass::RawPointerMove)
            }
            Self::Event(WireEvent::Scene {
                event: WireSceneEvent::PointerMove { .. },
            }) => Some(NotificationCoalescingClass::ScenePointerMove),
            Self::Event(WireEvent::Scene {
                event: WireSceneEvent::DragMove { .. },
            }) => Some(NotificationCoalescingClass::SceneDragMove),
            Self::Event(WireEvent::Scene {
                event: WireSceneEvent::ExternalDropOver { .. },
            }) => Some(NotificationCoalescingClass::SceneExternalDropOver),
            Self::Event(WireEvent::Terminal {
                state: WireTerminalState::OutputAvailable,
            }) => Some(NotificationCoalescingClass::TerminalOutputAvailable),
            Self::Event(_) | Self::Resize { .. } | Self::DeliveryGap { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NotificationCoalescingClass {
    Tick,
    Frame,
    RawPointerMove,
    ScenePointerMove,
    SceneDragMove,
    SceneExternalDropOver,
    TerminalOutputAvailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum DeliveryCategory {
    RawInput,
    Scene,
    Terminal,
    Timing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Subscription {
    RawInput,
    Terminal,
    Timing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SubscriptionError {
    CapabilityDenied,
    SessionUnavailable,
}

#[derive(Clone)]
struct SessionNotificationSender {
    capabilities: CapabilitySet,
    queue: Arc<NotificationQueue>,
    active: bool,
    subscriptions: BTreeSet<Subscription>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClpNotificationMetrics {
    pub coalesced_notifications: u64,
    pub dropped_continuous_notifications: u64,
    pub delivery_gaps_emitted: u64,
    pub overloaded_sessions: u64,
}

#[derive(Default)]
struct NotificationMetricCounters {
    coalesced_notifications: AtomicU64,
    dropped_continuous_notifications: AtomicU64,
    delivery_gaps_emitted: AtomicU64,
    overloaded_sessions: AtomicU64,
}

impl NotificationMetricCounters {
    fn snapshot(&self) -> ClpNotificationMetrics {
        ClpNotificationMetrics {
            coalesced_notifications: self.coalesced_notifications.load(Ordering::Relaxed),
            dropped_continuous_notifications: self
                .dropped_continuous_notifications
                .load(Ordering::Relaxed),
            delivery_gaps_emitted: self.delivery_gaps_emitted.load(Ordering::Relaxed),
            overloaded_sessions: self.overloaded_sessions.load(Ordering::Relaxed),
        }
    }
}

struct NotificationQueue {
    state: Mutex<NotificationQueueState>,
    metrics: Arc<NotificationMetricCounters>,
}

#[derive(Default)]
struct NotificationQueueState {
    notifications: VecDeque<HostNotification>,
    pending_gaps: BTreeSet<DeliveryCategory>,
    overloaded: bool,
}

enum NotificationPoll {
    Notification(HostNotification),
    Overloaded,
    Empty,
}

impl NotificationQueue {
    fn new(metrics: Arc<NotificationMetricCounters>) -> Self {
        Self {
            state: Mutex::new(NotificationQueueState::default()),
            metrics,
        }
    }

    fn push(&self, notification: HostNotification) {
        let mut state = self.lock_state();
        if state.overloaded {
            return;
        }
        if let Some(class) = notification.coalescing_class()
            && let Some(index) = state
                .notifications
                .iter()
                .rposition(|pending| pending.coalescing_class() == Some(class))
        {
            let replaced = state
                .notifications
                .remove(index)
                .expect("the pending notification index must be valid");
            Self::record_gap(&mut state.pending_gaps, replaced.delivery_category());
            self.metrics
                .coalesced_notifications
                .fetch_add(1, Ordering::Relaxed);
            state.notifications.push_back(notification);
            return;
        }

        if state.notifications.len() < MAX_PENDING_HOST_NOTIFICATIONS {
            state.notifications.push_back(notification);
            return;
        }

        if notification.coalescing_class().is_none()
            && let Some(index) = state
                .notifications
                .iter()
                .position(|pending| pending.coalescing_class().is_some())
        {
            let replaced = state
                .notifications
                .remove(index)
                .expect("the pending notification index must be valid");
            Self::record_gap(&mut state.pending_gaps, replaced.delivery_category());
            self.metrics
                .dropped_continuous_notifications
                .fetch_add(1, Ordering::Relaxed);
            state.notifications.push_back(notification);
            return;
        }

        if notification.coalescing_class().is_some() {
            Self::record_gap(&mut state.pending_gaps, notification.delivery_category());
            self.metrics
                .dropped_continuous_notifications
                .fetch_add(1, Ordering::Relaxed);
            return;
        }

        state.overloaded = true;
        self.metrics
            .overloaded_sessions
            .fetch_add(1, Ordering::Relaxed);
    }

    fn try_pop(&self) -> NotificationPoll {
        let mut state = self.lock_state();
        if state.overloaded {
            return NotificationPoll::Overloaded;
        }
        if !state.pending_gaps.is_empty() {
            self.metrics
                .delivery_gaps_emitted
                .fetch_add(1, Ordering::Relaxed);
            return NotificationPoll::Notification(HostNotification::DeliveryGap {
                categories: std::mem::take(&mut state.pending_gaps),
            });
        }
        match state.notifications.pop_front() {
            Some(notification) => NotificationPoll::Notification(notification),
            None => NotificationPoll::Empty,
        }
    }

    fn record_gap(
        pending_gaps: &mut BTreeSet<DeliveryCategory>,
        category: Option<DeliveryCategory>,
    ) {
        if let Some(category) = category {
            pending_gaps.insert(category);
        }
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, NotificationQueueState> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

#[derive(Default)]
struct SessionNotifications {
    sessions: Mutex<BTreeMap<SessionId, SessionNotificationSender>>,
    metrics: Arc<NotificationMetricCounters>,
}

impl SessionNotifications {
    fn register(
        &self,
        session_id: SessionId,
        capabilities: CapabilitySet,
    ) -> Arc<NotificationQueue> {
        let queue = Arc::new(NotificationQueue::new(Arc::clone(&self.metrics)));
        let mut sessions = self.lock_sessions();
        sessions.insert(
            session_id,
            SessionNotificationSender {
                capabilities,
                queue: Arc::clone(&queue),
                active: false,
                subscriptions: BTreeSet::new(),
            },
        );
        queue
    }

    fn unregister(&self, session_id: SessionId) {
        self.lock_sessions().remove(&session_id);
    }

    fn metrics(&self) -> ClpNotificationMetrics {
        self.metrics.snapshot()
    }

    fn activate(&self, session_id: SessionId, initial_resize: HostNotification) -> bool {
        let mut sessions = self.lock_sessions();
        let Some(session) = sessions.get_mut(&session_id) else {
            return false;
        };
        if session.active {
            return true;
        }
        if session.capabilities.contains(Capability::Input) {
            Self::enqueue(session, initial_resize);
        }
        session.active = true;
        true
    }

    fn set_subscriptions(
        &self,
        session_id: SessionId,
        subscriptions: BTreeSet<Subscription>,
    ) -> Result<BTreeSet<Subscription>, SubscriptionError> {
        let mut sessions = self.lock_sessions();
        let session = sessions
            .get_mut(&session_id)
            .ok_or(SubscriptionError::SessionUnavailable)?;
        if subscriptions.iter().any(|subscription| {
            let capability = match subscription {
                Subscription::RawInput | Subscription::Terminal => Capability::Input,
                Subscription::Timing => Capability::Timing,
            };
            !session.capabilities.contains(capability)
        }) {
            return Err(SubscriptionError::CapabilityDenied);
        }
        session.subscriptions = subscriptions;
        Ok(session.subscriptions.clone())
    }

    fn broadcast_matching(
        &self,
        notification: HostNotification,
        matches: impl Fn(&SessionNotificationSender) -> bool,
    ) {
        let mut sessions = self.lock_sessions();
        for session in sessions.values_mut() {
            if session.active && matches(session) {
                Self::enqueue(session, notification.clone());
            }
        }
    }

    fn send_matching(
        &self,
        session_id: SessionId,
        notification: HostNotification,
        matches: impl Fn(&SessionNotificationSender) -> bool,
    ) {
        let mut sessions = self.lock_sessions();
        if let Some(session) = sessions.get_mut(&session_id)
            && session.active
            && matches(session)
        {
            Self::enqueue(session, notification);
        }
    }

    fn enqueue(session: &SessionNotificationSender, notification: HostNotification) {
        session.queue.push(notification);
    }

    fn broadcast_scene(
        &self,
        event: &SceneEvent,
        node_handle_for: impl Fn(SessionId, NodeId) -> Option<NodeHandle>,
    ) {
        let sessions = self.lock_sessions();
        for (session_id, session) in sessions.iter() {
            if !session.active {
                continue;
            }
            let Some(event) = WireSceneEvent::from_scene_event(event, |node| {
                node_handle_for(*session_id, node).map(|handle| handle.to_string())
            }) else {
                continue;
            };
            if !session.capabilities.contains(event.required_capability()) {
                continue;
            }
            let notification = HostNotification::Event(WireEvent::Scene { event });
            Self::enqueue(session, notification);
        }
    }

    fn lock_sessions(
        &self,
    ) -> std::sync::MutexGuard<'_, BTreeMap<SessionId, SessionNotificationSender>> {
        match self.sessions.lock() {
            Ok(sessions) => sessions,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
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
        let (lifecycle_sender, lifecycle_actions) = mpsc::channel();
        let notifications = Arc::new(SessionNotifications::default());
        let broker = HostActionBroker {
            sender: host_action_sender,
            cleanup_sender,
            lifecycle_sender,
            wake: Arc::new(wake),
        };
        let listener_control = Arc::clone(&control);
        let listener_manager = Arc::clone(&manager);
        let listener_environment = environment.clone();
        let listener_broker = broker.clone();
        let listener_notifications = Arc::clone(&notifications);
        let listener_thread = thread::Builder::new()
            .name("chymeral-clp-listener".to_owned())
            .spawn(move || {
                serve_connections(
                    listener,
                    listener_environment,
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

    fn cleanup(&self, session_id: SessionId) {
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
                let connection_environment = environment.clone();
                let connection_manager = Arc::clone(&manager);
                let connection_control = Arc::clone(&control);
                let connection_broker = broker.clone();
                let connection_notifications = Arc::clone(&notifications);
                if let Err(error) = thread::Builder::new()
                    .name("chymeral-clp-client".to_owned())
                    .spawn(move || {
                        handle_connection(
                            stream,
                            connection_environment,
                            connection_manager,
                            connection_control,
                            connection_broker,
                            connection_notifications,
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
    notifications: Arc<SessionNotifications>,
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
                    match stream.write(&frame.bytes[frame.written..]) {
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
        WireSceneCommand::UploadImage { .. } | WireSceneCommand::RemoveImage { .. } => {
            capabilities.contains(Capability::Images)
        }
        WireSceneCommand::IsOverlapping { .. } | WireSceneCommand::GetOverlaps { .. } => {
            capabilities.contains(Capability::Scene) && capabilities.contains(Capability::Overlap)
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

fn write_frame<T: Serialize>(stream: &mut impl Write, message: &T) -> Result<()> {
    write_all_nonblocking(stream, &json_frame_bytes(message)?)
        .context("could not write the CLP JSON frame")?;
    Ok(())
}

#[cfg(test)]
fn write_binary_frame(stream: &mut impl Write, payload: &[u8]) -> Result<()> {
    write_raw_frame(stream, b'B', payload)
}

#[cfg(test)]
fn write_raw_frame(stream: &mut impl Write, kind: u8, payload: &[u8]) -> Result<()> {
    write_all_nonblocking(stream, &raw_frame_bytes(kind, payload)?)
        .context("could not write the CLP frame")?;
    Ok(())
}

fn json_frame_bytes(message: &impl Serialize) -> Result<Vec<u8>> {
    let payload = serde_json::to_vec(message).context("could not encode the CLP JSON message")?;
    raw_frame_bytes(b'J', &payload)
}

fn raw_frame_bytes(kind: u8, payload: &[u8]) -> Result<Vec<u8>> {
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
    let mut bytes = Vec::with_capacity(4 + frame_length);
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.push(kind);
    bytes.extend_from_slice(payload);
    Ok(bytes)
}

fn write_all_nonblocking(stream: &mut impl Write, mut bytes: &[u8]) -> io::Result<()> {
    let deadline = Instant::now() + WRITE_STALL_TIMEOUT;
    while !bytes.is_empty() {
        match stream.write(bytes) {
            Ok(0) => wait_for_write_capacity(deadline)?,
            Ok(written) => bytes = &bytes[written..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if read_would_block(&error) => {
                wait_for_write_capacity(deadline)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn wait_for_write_capacity(deadline: Instant) -> io::Result<()> {
    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "the CLP peer stopped reading output",
        ));
    };
    thread::sleep(remaining.min(WRITE_RETRY_INTERVAL));
    Ok(())
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
        command: Box<WireSceneCommand>,
    },
    Subscribe {
        subscriptions: Vec<WireSubscription>,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum WireSceneCommand {
    CreateNode {
        #[serde(default)]
        parent: Option<String>,
        #[serde(default)]
        layer: Option<WireSceneLayer>,
        node: WireNodeInput,
    },
    SetNode {
        node: String,
        changes: WireNodeChanges,
    },
    RemoveNode {
        node: String,
    },
    UploadImage {
        source: WireImageSource,
    },
    RemoveImage {
        image: String,
    },
    IsOverlapping {
        node: String,
    },
    GetOverlaps {
        node: String,
    },
}

impl WireSceneCommand {
    fn upload_length(&self) -> Result<Option<usize>> {
        match self {
            Self::UploadImage { source } => source.upload_length().map(Some),
            _ => Ok(None),
        }
    }

    fn into_clp_scene_command(self, pixels: Option<Vec<u8>>) -> Result<ClpSceneCommand> {
        match self {
            Self::CreateNode {
                parent,
                layer,
                node,
            } => Ok(ClpSceneCommand::CreateNode {
                parent: parent.as_deref().map(parse_node_handle).transpose()?,
                layer: layer.map(WireSceneLayer::into_scene),
                node: node.into_input()?,
            }),
            Self::SetNode { node, changes } => Ok(ClpSceneCommand::SetNode {
                node: parse_node_handle(&node)?,
                changes: changes.into_input()?,
            }),
            Self::RemoveNode { node } => Ok(ClpSceneCommand::RemoveNode {
                node: parse_node_handle(&node)?,
            }),
            Self::UploadImage { source } => Ok(ClpSceneCommand::UploadImage {
                image: source.into_input(
                    pixels.ok_or_else(|| anyhow!("an image upload requires binary image bytes"))?,
                )?,
            }),
            Self::RemoveImage { image } => Ok(ClpSceneCommand::RemoveImage {
                image: parse_image_handle(&image)?,
            }),
            Self::IsOverlapping { node } => Ok(ClpSceneCommand::IsOverlapping {
                node: parse_node_handle(&node)?,
            }),
            Self::GetOverlaps { node } => Ok(ClpSceneCommand::GetOverlaps {
                node: parse_node_handle(&node)?,
            }),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireImageSource {
    Rgba8 { width: u32, height: u32 },
    Gif { byte_length: usize },
}

impl WireImageSource {
    fn upload_length(&self) -> Result<usize> {
        match self {
            Self::Rgba8 { width, height } => {
                let bytes =
                    rgba8_byte_len(*width, *height).context("invalid RGBA8 image dimensions")?;
                if bytes > MAX_TEXTURE_BYTES {
                    return Err(anyhow!(
                        "an RGBA8 image may not exceed {MAX_TEXTURE_BYTES} bytes"
                    ));
                }
                Ok(bytes)
            }
            Self::Gif { byte_length } => {
                if *byte_length > MAX_ANIMATED_IMAGE_ENCODED_BYTES {
                    return Err(anyhow!(
                        "an encoded GIF image may not exceed {MAX_ANIMATED_IMAGE_ENCODED_BYTES} bytes"
                    ));
                }
                Ok(*byte_length)
            }
        }
    }

    fn into_input(self, bytes: Vec<u8>) -> Result<ImageUpload> {
        match self {
            Self::Rgba8 { width, height } => Ok(ImageUpload::Rgba8 {
                width,
                height,
                pixels: bytes,
            }),
            Self::Gif { .. } => Ok(ImageUpload::Gif { bytes }),
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
    #[serde(default)]
    overlap_enabled: bool,
    #[serde(default)]
    external_drop_enabled: bool,
    #[serde(default)]
    input_passthrough: bool,
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
            overlap_enabled: self.overlap_enabled,
            external_drop_enabled: self.external_drop_enabled,
            input_passthrough: self.input_passthrough,
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
    overlap_enabled: Option<bool>,
    external_drop_enabled: Option<bool>,
    input_passthrough: Option<bool>,
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
            overlap_enabled: self.overlap_enabled,
            external_drop_enabled: self.external_drop_enabled,
            input_passthrough: self.input_passthrough,
        }
    }
}

#[derive(Deserialize, Serialize)]
struct WireTextNodeProperties {
    x: f64,
    y: f64,
    #[serde(default = "default_visible")]
    visible: bool,
    #[serde(default)]
    clip_children: bool,
    #[serde(default)]
    z_index: i32,
    #[serde(default)]
    overlap_enabled: bool,
    #[serde(default)]
    external_drop_enabled: bool,
    #[serde(default)]
    input_passthrough: bool,
}

impl WireTextNodeProperties {
    fn into_input(self) -> TextNodePropertiesInput {
        TextNodePropertiesInput {
            x: self.x,
            y: self.y,
            visible: self.visible,
            clip_children: self.clip_children,
            z_index: self.z_index,
            overlap_enabled: self.overlap_enabled,
            external_drop_enabled: self.external_drop_enabled,
            input_passthrough: self.input_passthrough,
        }
    }
}

#[derive(Default, Deserialize, Serialize)]
struct WireTextStyle {
    #[serde(default)]
    font: Option<String>,
    #[serde(default)]
    size: Option<f32>,
    #[serde(default)]
    line_height: Option<f32>,
    #[serde(default)]
    color: Option<[u8; 4]>,
    #[serde(default)]
    weight: WireTextWeight,
    #[serde(default)]
    align: WireTextAlign,
}

impl WireTextStyle {
    fn into_scene(self) -> TextStyle {
        TextStyle {
            font_alias: self.font,
            size: self.size,
            line_height: self.line_height,
            color: self.color,
            weight: self.weight.into_scene(),
            align: self.align.into_scene(),
        }
    }
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireTextWeight {
    #[default]
    Normal,
    Bold,
}

impl WireTextWeight {
    const fn into_scene(self) -> TextWeight {
        match self {
            Self::Normal => TextWeight::Normal,
            Self::Bold => TextWeight::Bold,
        }
    }
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireTextAlign {
    #[default]
    Start,
    Center,
    End,
}

impl WireTextAlign {
    const fn into_scene(self) -> TextAlign {
        match self {
            Self::Start => TextAlign::Start,
            Self::Center => TextAlign::Center,
            Self::End => TextAlign::End,
        }
    }
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireImageContactShape {
    #[default]
    Bounds,
    Alpha {
        threshold: u8,
    },
}

impl WireImageContactShape {
    const fn into_scene(self) -> ImageContactShape {
        match self {
            Self::Bounds => ImageContactShape::Bounds,
            Self::Alpha { threshold } => ImageContactShape::Alpha { threshold },
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireNodeInput {
    Empty {
        properties: WireNodeProperties,
    },
    Rectangle {
        properties: WireNodeProperties,
        color: [u8; 4],
    },
    Image {
        properties: WireNodeProperties,
        image: String,
        opacity: f32,
        #[serde(default)]
        contact_shape: WireImageContactShape,
    },
    Text {
        properties: WireTextNodeProperties,
        text: String,
        #[serde(default)]
        style: WireTextStyle,
    },
}

impl WireNodeInput {
    fn into_input(self) -> Result<NodeInput> {
        match self {
            Self::Empty { properties } => Ok(NodeInput::Empty {
                properties: properties.into_input(),
            }),
            Self::Rectangle { properties, color } => Ok(NodeInput::Rectangle {
                properties: properties.into_input(),
                color,
            }),
            Self::Image {
                properties,
                image,
                opacity,
                contact_shape,
            } => Ok(NodeInput::Image {
                properties: properties.into_input(),
                image: parse_image_handle(&image)?,
                opacity,
                contact_shape: contact_shape.into_scene(),
            }),
            Self::Text {
                properties,
                text,
                style,
            } => Ok(NodeInput::Text {
                properties: properties.into_input(),
                text,
                style: style.into_scene(),
            }),
        }
    }

    const fn uses_image(&self) -> bool {
        matches!(self, Self::Image { .. })
    }

    const fn uses_text(&self) -> bool {
        matches!(self, Self::Text { .. })
    }

    fn uses_overlap(&self) -> bool {
        match self {
            Self::Empty { properties }
            | Self::Rectangle { properties, .. }
            | Self::Image { properties, .. } => properties.overlap_enabled,
            Self::Text { properties, .. } => properties.overlap_enabled,
        }
    }

    fn uses_external_drop(&self) -> bool {
        match self {
            Self::Empty { properties }
            | Self::Rectangle { properties, .. }
            | Self::Image { properties, .. } => properties.external_drop_enabled,
            Self::Text { properties, .. } => properties.external_drop_enabled,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireNodeVisual {
    Empty,
    Rectangle {
        color: [u8; 4],
    },
    Image {
        image: String,
        opacity: f32,
        #[serde(default)]
        contact_shape: WireImageContactShape,
    },
}

impl WireNodeVisual {
    fn into_input(self) -> Result<NodeVisualInput> {
        match self {
            Self::Empty => Ok(NodeVisualInput::Empty),
            Self::Rectangle { color } => Ok(NodeVisualInput::Rectangle { color }),
            Self::Image {
                image,
                opacity,
                contact_shape,
            } => Ok(NodeVisualInput::Image {
                image: parse_image_handle(&image)?,
                opacity,
                contact_shape: contact_shape.into_scene(),
            }),
        }
    }

    const fn uses_image(&self) -> bool {
        matches!(self, Self::Image { .. })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireNodeParent {
    Root,
    Node { node: String },
}

impl WireNodeParent {
    fn into_input(self) -> Result<NodeParent> {
        match self {
            Self::Root => Ok(NodeParent::Root),
            Self::Node { node } => Ok(NodeParent::Node(parse_node_handle(&node)?)),
        }
    }
}

#[derive(Default, Deserialize, Serialize)]
struct WireNodeChanges {
    #[serde(default)]
    properties: Option<WireNodePropertiesPatch>,
    #[serde(default)]
    parent: Option<WireNodeParent>,
    #[serde(default)]
    layer: Option<WireSceneLayer>,
    #[serde(default)]
    visual: Option<WireNodeVisual>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    text_style: Option<WireTextStyle>,
}

impl WireNodeChanges {
    fn into_input(self) -> Result<NodeChanges> {
        Ok(NodeChanges {
            properties: self.properties.map(WireNodePropertiesPatch::into_patch),
            parent: self.parent.map(WireNodeParent::into_input).transpose()?,
            layer: self.layer.map(WireSceneLayer::into_scene),
            visual: self.visual.map(WireNodeVisual::into_input).transpose()?,
            text: self.text,
            text_style: self.text_style.map(WireTextStyle::into_scene),
        })
    }

    fn uses_image(&self) -> bool {
        self.visual.as_ref().is_some_and(WireNodeVisual::uses_image)
    }

    const fn uses_text(&self) -> bool {
        self.text.is_some() || self.text_style.is_some()
    }

    fn uses_overlap(&self) -> bool {
        self.properties
            .as_ref()
            .is_some_and(|properties| properties.overlap_enabled.is_some())
    }

    fn uses_external_drop(&self) -> bool {
        self.properties
            .as_ref()
            .is_some_and(|properties| properties.external_drop_enabled.is_some())
    }
}

fn parse_node_handle(value: &str) -> Result<NodeHandle> {
    parse_handle(value, "node-").map(NodeHandle::new)
}

fn parse_image_handle(value: &str) -> Result<ImageHandle> {
    parse_handle(value, "image-").map(ImageHandle::new)
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
    Images,
    Input,
    Timing,
    Overlap,
    ExternalDrop,
    Text,
}

impl WireCapability {
    const fn into_protocol(self) -> Capability {
        match self {
            Self::Scene => Capability::Scene,
            Self::Images => Capability::Images,
            Self::Input => Capability::Input,
            Self::Timing => Capability::Timing,
            Self::Overlap => Capability::Overlap,
            Self::ExternalDrop => Capability::ExternalDrop,
            Self::Text => Capability::Text,
        }
    }
}

impl From<Capability> for WireCapability {
    fn from(capability: Capability) -> Self {
        match capability {
            Capability::Scene => Self::Scene,
            Capability::Images => Self::Images,
            Capability::Input => Self::Input,
            Capability::Timing => Self::Timing,
            Capability::Overlap => Self::Overlap,
            Capability::ExternalDrop => Self::ExternalDrop,
            Capability::Text => Self::Text,
        }
    }
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum WireSubscription {
    RawInput,
    Terminal,
    Timing,
}

impl WireSubscription {
    const fn into_subscription(self) -> Subscription {
        match self {
            Self::RawInput => Subscription::RawInput,
            Self::Terminal => Subscription::Terminal,
            Self::Timing => Subscription::Timing,
        }
    }
}

impl From<Subscription> for WireSubscription {
    fn from(subscription: Subscription) -> Self {
        match subscription {
            Subscription::RawInput => Self::RawInput,
            Subscription::Terminal => Self::Terminal,
            Subscription::Timing => Self::Timing,
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
    Event {
        session_id: String,
        event: WireEvent,
    },
    Tick {
        session_id: String,
        dt_ns: String,
    },
    Frame {
        session_id: String,
        dt_ns: String,
    },
    Resize {
        session_id: String,
        physical_width: u32,
        physical_height: u32,
        logical_width: f64,
        logical_height: f64,
        scale_factor: f64,
    },
    Subscribed {
        session_id: String,
        subscriptions: Vec<WireSubscription>,
    },
    DeliveryGap {
        session_id: String,
        categories: Vec<WireDeliveryCategory>,
    },
    Closing {
        session_id: String,
        reason: WireCloseReason,
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

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireEvent {
    Key {
        key: WireKey,
        text: Option<String>,
        state: WireKeyState,
        modifiers: WireModifiers,
        repeat: bool,
    },
    PointerMove {
        position: WireVec2,
        modifiers: WireModifiers,
    },
    PointerPress {
        button: WirePointerButton,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    PointerRelease {
        button: WirePointerButton,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    Scroll {
        position: WireVec2,
        delta: WireVec2,
        unit: WireScrollUnit,
        modifiers: WireModifiers,
    },
    Focus {
        state: WireFocusState,
    },
    Scene {
        event: WireSceneEvent,
    },
    Terminal {
        state: WireTerminalState,
    },
    Custom {
        name: String,
        payload: Vec<u8>,
    },
}

impl WireEvent {
    fn from_engine_event(event: &EngineEvent) -> Option<Self> {
        match event {
            EngineEvent::Key(event) => Some(Self::Key {
                key: event.key.clone().into(),
                text: event.text.clone(),
                state: event.state.into(),
                modifiers: event.modifiers.into(),
                repeat: event.repeat,
            }),
            EngineEvent::Pointer(PointerEvent::Move {
                position,
                modifiers,
            }) => Some(Self::PointerMove {
                position: (*position).into(),
                modifiers: (*modifiers).into(),
            }),
            EngineEvent::Pointer(PointerEvent::Press {
                button,
                position,
                modifiers,
            }) => Some(Self::PointerPress {
                button: (*button).into(),
                position: (*position).into(),
                modifiers: (*modifiers).into(),
            }),
            EngineEvent::Pointer(PointerEvent::Release {
                button,
                position,
                modifiers,
            }) => Some(Self::PointerRelease {
                button: (*button).into(),
                position: (*position).into(),
                modifiers: (*modifiers).into(),
            }),
            EngineEvent::Scroll(ScrollEvent {
                position,
                delta,
                unit,
                modifiers,
            }) => Some(Self::Scroll {
                position: (*position).into(),
                delta: (*delta).into(),
                unit: (*unit).into(),
                modifiers: (*modifiers).into(),
            }),
            EngineEvent::Focus(event) => Some(Self::Focus {
                state: (*event).into(),
            }),
            EngineEvent::Scene(_) => None,
            EngineEvent::Terminal(event) => Some(Self::Terminal {
                state: event.clone().into(),
            }),
            EngineEvent::Custom(event) => Some(Self::Custom {
                name: event.name.clone(),
                payload: event.payload.clone(),
            }),
        }
    }

    const fn delivery_category(&self) -> Option<DeliveryCategory> {
        match self {
            Self::PointerMove { .. } => Some(DeliveryCategory::RawInput),
            Self::Terminal {
                state: WireTerminalState::OutputAvailable,
            } => Some(DeliveryCategory::Terminal),
            Self::Scene {
                event:
                    WireSceneEvent::PointerMove { .. }
                    | WireSceneEvent::DragMove { .. }
                    | WireSceneEvent::ExternalDropOver { .. },
            } => Some(DeliveryCategory::Scene),
            Self::Key { .. }
            | Self::PointerPress { .. }
            | Self::PointerRelease { .. }
            | Self::Scroll { .. }
            | Self::Focus { .. }
            | Self::Custom { .. } => Some(DeliveryCategory::RawInput),
            Self::Terminal {
                state: WireTerminalState::Exited,
            } => Some(DeliveryCategory::Terminal),
            Self::Scene { .. } => Some(DeliveryCategory::Scene),
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireDeliveryCategory {
    RawInput,
    Scene,
    Terminal,
    Timing,
}

impl From<DeliveryCategory> for WireDeliveryCategory {
    fn from(category: DeliveryCategory) -> Self {
        match category {
            DeliveryCategory::RawInput => Self::RawInput,
            DeliveryCategory::Scene => Self::Scene,
            DeliveryCategory::Terminal => Self::Terminal,
            DeliveryCategory::Timing => Self::Timing,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireSceneEvent {
    PointerEnter {
        target: String,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    PointerLeave {
        target: String,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    PointerMove {
        target: String,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    PointerPress {
        target: String,
        button: WirePointerButton,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    PointerRelease {
        target: String,
        button: WirePointerButton,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    FocusGained {
        target: String,
    },
    FocusLost {
        target: String,
    },
    DragStart {
        source: String,
        origin: WireVec2,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    DragEnter {
        source: Option<String>,
        target: Option<String>,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    DragLeave {
        source: Option<String>,
        target: Option<String>,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    DragMove {
        source: Option<String>,
        target: Option<String>,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    Drop {
        source: Option<String>,
        target: Option<String>,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    DragEnd {
        source: Option<String>,
        target: Option<String>,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    DragCancel {
        source: String,
        position: WireVec2,
        modifiers: WireModifiers,
    },
    OverlapEnter {
        first: String,
        second: String,
    },
    OverlapLeave {
        first: String,
        second: String,
    },
    ExternalDropEnter {
        target: String,
        position: WireVec2,
    },
    ExternalDropOver {
        target: String,
        position: WireVec2,
    },
    ExternalDropLeave {
        target: String,
        position: WireVec2,
    },
    ExternalDrop {
        target: String,
        position: WireVec2,
        files: Vec<WireExternalFile>,
    },
}

#[derive(Clone, Serialize)]
struct WireExternalFile {
    uri: String,
}

impl WireSceneEvent {
    fn from_scene_event(
        event: &SceneEvent,
        mut node_handle_for: impl FnMut(NodeId) -> Option<String>,
    ) -> Option<Self> {
        match event {
            SceneEvent::PointerEnter {
                target,
                position,
                modifiers,
            } => Some(Self::PointerEnter {
                target: node_handle_for(*target)?,
                position: (*position).into(),
                modifiers: (*modifiers).into(),
            }),
            SceneEvent::PointerLeave {
                target,
                position,
                modifiers,
            } => Some(Self::PointerLeave {
                target: node_handle_for(*target)?,
                position: (*position).into(),
                modifiers: (*modifiers).into(),
            }),
            SceneEvent::PointerMove {
                target,
                position,
                modifiers,
            } => Some(Self::PointerMove {
                target: node_handle_for(*target)?,
                position: (*position).into(),
                modifiers: (*modifiers).into(),
            }),
            SceneEvent::PointerPress {
                target,
                button,
                position,
                modifiers,
            } => Some(Self::PointerPress {
                target: node_handle_for(*target)?,
                button: (*button).into(),
                position: (*position).into(),
                modifiers: (*modifiers).into(),
            }),
            SceneEvent::PointerRelease {
                target,
                button,
                position,
                modifiers,
            } => Some(Self::PointerRelease {
                target: node_handle_for(*target)?,
                button: (*button).into(),
                position: (*position).into(),
                modifiers: (*modifiers).into(),
            }),
            SceneEvent::FocusGained { target } => Some(Self::FocusGained {
                target: node_handle_for(*target)?,
            }),
            SceneEvent::FocusLost { target } => Some(Self::FocusLost {
                target: node_handle_for(*target)?,
            }),
            SceneEvent::DragStart {
                source,
                origin,
                position,
                modifiers,
            } => Some(Self::DragStart {
                source: node_handle_for(*source)?,
                origin: (*origin).into(),
                position: (*position).into(),
                modifiers: (*modifiers).into(),
            }),
            SceneEvent::DragEnter {
                source,
                target,
                position,
                modifiers,
            } => Self::with_drag_handles(
                node_handle_for(*source),
                node_handle_for(*target),
                |source, target| Self::DragEnter {
                    source,
                    target,
                    position: (*position).into(),
                    modifiers: (*modifiers).into(),
                },
            ),
            SceneEvent::DragLeave {
                source,
                target,
                position,
                modifiers,
            } => Self::with_drag_handles(
                node_handle_for(*source),
                node_handle_for(*target),
                |source, target| Self::DragLeave {
                    source,
                    target,
                    position: (*position).into(),
                    modifiers: (*modifiers).into(),
                },
            ),
            SceneEvent::DragMove {
                source,
                target,
                position,
                modifiers,
            } => Self::with_drag_handles(
                node_handle_for(*source),
                target.and_then(&mut node_handle_for),
                |source, target| Self::DragMove {
                    source,
                    target,
                    position: (*position).into(),
                    modifiers: (*modifiers).into(),
                },
            ),
            SceneEvent::Drop {
                source,
                target,
                position,
                modifiers,
            } => Self::with_drag_handles(
                node_handle_for(*source),
                node_handle_for(*target),
                |source, target| Self::Drop {
                    source,
                    target,
                    position: (*position).into(),
                    modifiers: (*modifiers).into(),
                },
            ),
            SceneEvent::DragEnd {
                source,
                target,
                position,
                modifiers,
            } => Self::with_drag_handles(
                node_handle_for(*source),
                target.and_then(&mut node_handle_for),
                |source, target| Self::DragEnd {
                    source,
                    target,
                    position: (*position).into(),
                    modifiers: (*modifiers).into(),
                },
            ),
            SceneEvent::DragCancel {
                source,
                position,
                modifiers,
            } => Some(Self::DragCancel {
                source: node_handle_for(*source)?,
                position: (*position).into(),
                modifiers: (*modifiers).into(),
            }),
            SceneEvent::OverlapEnter { first, second } => Some(Self::OverlapEnter {
                first: node_handle_for(*first)?,
                second: node_handle_for(*second)?,
            }),
            SceneEvent::OverlapLeave { first, second } => Some(Self::OverlapLeave {
                first: node_handle_for(*first)?,
                second: node_handle_for(*second)?,
            }),
            SceneEvent::ExternalDropEnter { target, position } => Some(Self::ExternalDropEnter {
                target: node_handle_for(*target)?,
                position: (*position).into(),
            }),
            SceneEvent::ExternalDropOver { target, position } => Some(Self::ExternalDropOver {
                target: node_handle_for(*target)?,
                position: (*position).into(),
            }),
            SceneEvent::ExternalDropLeave { target, position } => Some(Self::ExternalDropLeave {
                target: node_handle_for(*target)?,
                position: (*position).into(),
            }),
            SceneEvent::ExternalDrop {
                target,
                position,
                files,
            } => Some(Self::ExternalDrop {
                target: node_handle_for(*target)?,
                position: (*position).into(),
                files: files
                    .iter()
                    .map(|file| WireExternalFile {
                        uri: file.uri.clone(),
                    })
                    .collect(),
            }),
        }
    }

    const fn required_capability(&self) -> Capability {
        match self {
            Self::OverlapEnter { .. } | Self::OverlapLeave { .. } => Capability::Overlap,
            Self::ExternalDropEnter { .. }
            | Self::ExternalDropOver { .. }
            | Self::ExternalDropLeave { .. }
            | Self::ExternalDrop { .. } => Capability::ExternalDrop,
            Self::PointerEnter { .. }
            | Self::PointerLeave { .. }
            | Self::PointerMove { .. }
            | Self::PointerPress { .. }
            | Self::PointerRelease { .. }
            | Self::FocusGained { .. }
            | Self::FocusLost { .. }
            | Self::DragStart { .. }
            | Self::DragEnter { .. }
            | Self::DragLeave { .. }
            | Self::DragMove { .. }
            | Self::Drop { .. }
            | Self::DragEnd { .. }
            | Self::DragCancel { .. } => Capability::Input,
        }
    }

    fn with_drag_handles(
        source: Option<String>,
        target: Option<String>,
        build: impl FnOnce(Option<String>, Option<String>) -> Self,
    ) -> Option<Self> {
        (source.is_some() || target.is_some()).then(|| build(source, target))
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireTerminalState {
    OutputAvailable,
    Exited,
}

impl From<TerminalEvent> for WireTerminalState {
    fn from(event: TerminalEvent) -> Self {
        match event {
            TerminalEvent::OutputAvailable => Self::OutputAvailable,
            TerminalEvent::Exited => Self::Exited,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireKey {
    Character { value: String },
    Named { value: String },
    Dead { value: Option<String> },
    Unidentified,
}

impl From<Key> for WireKey {
    fn from(key: Key) -> Self {
        match key {
            Key::Character(value) => Self::Character { value },
            Key::Named(key) => Self::Named {
                value: wire_named_key(key),
            },
            Key::Dead(value) => Self::Dead {
                value: value.map(|character| character.to_string()),
            },
            Key::Unidentified => Self::Unidentified,
        }
    }
}

fn wire_named_key(key: NamedKey) -> String {
    match key {
        NamedKey::Alt => "alt".to_owned(),
        NamedKey::Backspace => "backspace".to_owned(),
        NamedKey::Delete => "delete".to_owned(),
        NamedKey::End => "end".to_owned(),
        NamedKey::Enter => "enter".to_owned(),
        NamedKey::Escape => "escape".to_owned(),
        NamedKey::Home => "home".to_owned(),
        NamedKey::Insert => "insert".to_owned(),
        NamedKey::PageDown => "page_down".to_owned(),
        NamedKey::PageUp => "page_up".to_owned(),
        NamedKey::Space => "space".to_owned(),
        NamedKey::Tab => "tab".to_owned(),
        NamedKey::ArrowDown => "arrow_down".to_owned(),
        NamedKey::ArrowLeft => "arrow_left".to_owned(),
        NamedKey::ArrowRight => "arrow_right".to_owned(),
        NamedKey::ArrowUp => "arrow_up".to_owned(),
        NamedKey::Other(value) => value,
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireKeyState {
    Pressed,
    Released,
}

impl From<KeyState> for WireKeyState {
    fn from(state: KeyState) -> Self {
        match state {
            KeyState::Pressed => Self::Pressed,
            KeyState::Released => Self::Released,
        }
    }
}

#[derive(Clone, Copy, Serialize)]
struct WireModifiers {
    shift: bool,
    control: bool,
    alt: bool,
    super_key: bool,
}

impl From<Modifiers> for WireModifiers {
    fn from(modifiers: Modifiers) -> Self {
        Self {
            shift: modifiers.shift,
            control: modifiers.control,
            alt: modifiers.alt,
            super_key: modifiers.super_key,
        }
    }
}

#[derive(Clone, Copy, Serialize)]
struct WireVec2 {
    x: f64,
    y: f64,
}

impl From<Vec2> for WireVec2 {
    fn from(position: Vec2) -> Self {
        Self {
            x: position.x,
            y: position.y,
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WirePointerButton {
    Primary,
    Secondary,
    Middle,
    Back,
    Forward,
    Other { value: u16 },
}

impl From<PointerButton> for WirePointerButton {
    fn from(button: PointerButton) -> Self {
        match button {
            PointerButton::Primary => Self::Primary,
            PointerButton::Secondary => Self::Secondary,
            PointerButton::Middle => Self::Middle,
            PointerButton::Back => Self::Back,
            PointerButton::Forward => Self::Forward,
            PointerButton::Other(value) => Self::Other { value },
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireScrollUnit {
    Line,
    Pixel,
}

impl From<ScrollUnit> for WireScrollUnit {
    fn from(unit: ScrollUnit) -> Self {
        match unit {
            ScrollUnit::Line => Self::Line,
            ScrollUnit::Pixel => Self::Pixel,
        }
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum WireFocusState {
    Gained,
    Lost,
}

impl From<FocusEvent> for WireFocusState {
    fn from(event: FocusEvent) -> Self {
        match event {
            FocusEvent::Gained => Self::Gained,
            FocusEvent::Lost => Self::Lost,
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireCommandResult {
    Node {
        node: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        text_metrics: Option<WireTextMetrics>,
    },
    NodeUpdated {
        #[serde(skip_serializing_if = "Option::is_none")]
        text_metrics: Option<WireTextMetrics>,
    },
    Image {
        image: String,
    },
    IsOverlapping {
        overlapping: bool,
    },
    Overlaps {
        nodes: Vec<String>,
    },
    Acknowledged,
}

impl From<ClpSceneCommandResult> for WireCommandResult {
    fn from(result: ClpSceneCommandResult) -> Self {
        match result {
            ClpSceneCommandResult::Node { node, text_metrics } => Self::Node {
                node: node.to_string(),
                text_metrics: text_metrics.map(Into::into),
            },
            ClpSceneCommandResult::NodeUpdated { text_metrics } => Self::NodeUpdated {
                text_metrics: text_metrics.map(Into::into),
            },
            ClpSceneCommandResult::Image(image) => Self::Image {
                image: image.to_string(),
            },
            ClpSceneCommandResult::IsOverlapping(overlapping) => {
                Self::IsOverlapping { overlapping }
            }
            ClpSceneCommandResult::Overlaps(nodes) => Self::Overlaps {
                nodes: nodes.into_iter().map(|node| node.to_string()).collect(),
            },
            ClpSceneCommandResult::Acknowledged => Self::Acknowledged,
        }
    }
}

#[derive(Serialize)]
struct WireTextMetrics {
    columns: u32,
    rows: u32,
    width: f64,
    height: f64,
}

impl From<TextMetrics> for WireTextMetrics {
    fn from(metrics: TextMetrics) -> Self {
        Self {
            columns: metrics.columns,
            rows: metrics.rows,
            width: metrics.width,
            height: metrics.height,
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

impl WireHostMessage {
    fn from_notification(session_id: SessionId, notification: HostNotification) -> Self {
        let session_id = session_id.to_string();
        match notification {
            HostNotification::Event(event) => Self::Event { session_id, event },
            HostNotification::Tick { dt_ns } => Self::Tick { session_id, dt_ns },
            HostNotification::Frame { dt_ns } => Self::Frame { session_id, dt_ns },
            HostNotification::Resize {
                physical_width,
                physical_height,
                logical_width,
                logical_height,
                scale_factor,
            } => Self::Resize {
                session_id,
                physical_width,
                physical_height,
                logical_width,
                logical_height,
                scale_factor,
            },
            HostNotification::DeliveryGap { categories } => Self::DeliveryGap {
                session_id,
                categories: categories
                    .into_iter()
                    .map(WireDeliveryCategory::from)
                    .collect(),
            },
        }
    }
}

fn capabilities_to_wire(capabilities: CapabilitySet) -> Vec<WireCapability> {
    [
        Capability::Scene,
        Capability::Images,
        Capability::Input,
        Capability::Timing,
        Capability::Overlap,
        Capability::ExternalDrop,
        Capability::Text,
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
    HostOverloaded,
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
            SessionCloseReason::HostOverloaded => Self::HostOverloaded,
            SessionCloseReason::ProtocolViolation => Self::ProtocolViolation,
            SessionCloseReason::TransportDisconnected => Self::TransportDisconnected,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::{Duration, Instant};

    use interprocess::local_socket::{GenericNamespaced, ToNsName, prelude::LocalSocketStream};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    use crate::protocol::{ClientHello, ProgramInitContext, SessionManager};
    use crate::runtime::KeyEvent;

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
        let fixture_bytes = include_bytes!("../protocol/fixtures/v2.0/fixtures.json");
        let expected_hash = include_str!("../protocol/fixtures/v2.0/SHA256SUMS")
            .split_whitespace()
            .next()
            .expect("fixture SHA256SUMS must contain a checksum");
        assert_eq!(
            format!("{:x}", Sha256::digest(fixture_bytes)),
            expected_hash
        );

        let corpus: FixtureCorpus =
            serde_json::from_str(include_str!("../protocol/fixtures/v2.0/fixtures.json"))
                .expect("the canonical CLP fixture corpus must be valid JSON");

        assert_eq!(corpus.format, "chymeral-clp-fixture-corpus");
        assert_eq!(corpus.fixture_version, 1);
        assert_eq!(
            (corpus.protocol.major, corpus.protocol.minor),
            (
                crate::protocol::CLP_PROTOCOL_VERSION.major(),
                crate::protocol::CLP_PROTOCOL_VERSION.minor()
            )
        );
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
}

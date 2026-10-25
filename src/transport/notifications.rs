//! Per-session CLP notification scheduling and delivery accounting.
//!
//! Continuous input and timing updates may coalesce, while structural and
//! lifecycle notifications retain ordering. A bounded queue prevents an
//! unread client from retaining unbounded host memory.

use super::*;

#[derive(Clone)]
pub(super) enum HostNotification {
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
    PresentationStatus {
        presentation_id: String,
        status: PresentationStatus,
    },
    DeliveryGap {
        categories: BTreeSet<DeliveryCategory>,
    },
}

#[derive(Clone)]
pub(super) enum PresentationStatus {
    Presented,
    Rejected { code: String, message: String },
    Superseded,
}

impl HostNotification {
    const fn delivery_category(&self) -> Option<DeliveryCategory> {
        match self {
            Self::Event(event) => event.delivery_category(),
            Self::Tick { .. } | Self::Frame { .. } => Some(DeliveryCategory::Timing),
            Self::Resize { .. } | Self::PresentationStatus { .. } | Self::DeliveryGap { .. } => {
                None
            }
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
            Self::PresentationStatus { .. } => {
                Some(NotificationCoalescingClass::PresentationStatus)
            }
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
    PresentationStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum DeliveryCategory {
    RawInput,
    Scene,
    Terminal,
    Timing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Subscription {
    RawInput,
    Terminal,
    Timing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SubscriptionError {
    CapabilityDenied,
    SessionUnavailable,
}

#[derive(Clone)]
pub(super) struct SessionNotificationSender {
    pub(super) capabilities: CapabilitySet,
    queue: Arc<NotificationQueue>,
    active: bool,
    pub(super) subscriptions: BTreeSet<Subscription>,
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

pub(super) struct NotificationQueue {
    state: Mutex<NotificationQueueState>,
    metrics: Arc<NotificationMetricCounters>,
}

#[derive(Default)]
struct NotificationQueueState {
    notifications: VecDeque<HostNotification>,
    pending_gaps: BTreeSet<DeliveryCategory>,
    overloaded: bool,
}

pub(super) enum NotificationPoll {
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

    pub(super) fn try_pop(&self) -> NotificationPoll {
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
pub(super) struct SessionNotifications {
    sessions: Mutex<BTreeMap<SessionId, SessionNotificationSender>>,
    metrics: Arc<NotificationMetricCounters>,
}

impl SessionNotifications {
    pub(super) fn register(
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

    pub(super) fn unregister(&self, session_id: SessionId) {
        self.lock_sessions().remove(&session_id);
    }

    pub(super) fn metrics(&self) -> ClpNotificationMetrics {
        self.metrics.snapshot()
    }

    pub(super) fn activate(&self, session_id: SessionId, initial_resize: HostNotification) -> bool {
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

    pub(super) fn set_subscriptions(
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

    pub(super) fn broadcast_matching(
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

    pub(super) fn send_matching(
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

    pub(super) fn broadcast_scene(
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

pub(super) fn enqueue_presentation_status(
    notifications: &SessionNotifications,
    session_id: SessionId,
    presentation_id: String,
    status: PresentationStatus,
) {
    notifications.send_matching(
        session_id,
        HostNotification::PresentationStatus {
            presentation_id,
            status,
        },
        |session| session.capabilities.contains(Capability::Presentation),
    );
}

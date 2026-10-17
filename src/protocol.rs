use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt::{Display, Formatter},
};

pub const CLP_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::new(2, 0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProtocolVersion {
    major: u16,
    minor: u16,
}

impl ProtocolVersion {
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    pub const fn major(self) -> u16 {
        self.major
    }

    pub const fn minor(self) -> u16 {
        self.minor
    }
}

impl Display for ProtocolVersion {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Capability {
    Scene,
    Images,
    Input,
    Timing,
    Overlap,
    ExternalDrop,
    Text,
}

impl Capability {
    const fn bit(self) -> u8 {
        match self {
            Self::Scene => 1 << 0,
            Self::Images => 1 << 1,
            Self::Input => 1 << 2,
            Self::Timing => 1 << 3,
            Self::Overlap => 1 << 4,
            Self::ExternalDrop => 1 << 5,
            Self::Text => 1 << 6,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CapabilitySet(u8);

impl CapabilitySet {
    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self(
        Capability::Scene.bit()
            | Capability::Images.bit()
            | Capability::Input.bit()
            | Capability::Timing.bit()
            | Capability::Overlap.bit()
            | Capability::ExternalDrop.bit()
            | Capability::Text.bit(),
    );

    pub const fn from_capabilities(capabilities: &[Capability]) -> Self {
        let mut bits = 0;
        let mut index = 0;
        while index < capabilities.len() {
            bits |= capabilities[index].bit();
            index += 1;
        }
        Self(bits)
    }

    pub const fn contains(self, capability: Capability) -> bool {
        self.0 & capability.bit() != 0
    }

    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub const fn without(self, capability: Capability) -> Self {
        Self(self.0 & !capability.bit())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientHello {
    pub protocol_version: ProtocolVersion,
    pub requested_capabilities: CapabilitySet,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupCommand {
    pub name: String,
    pub arguments: Vec<String>,
}

impl StartupCommand {
    pub fn new(name: impl Into<String>, arguments: impl IntoIterator<Item = String>) -> Self {
        Self {
            name: name.into(),
            arguments: arguments.into_iter().collect(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProgramInitContext {
    pub arguments: Vec<String>,
    pub initial_commands: Vec<StartupCommand>,
}

impl ProgramInitContext {
    pub fn new(
        arguments: impl IntoIterator<Item = String>,
        initial_commands: impl IntoIterator<Item = StartupCommand>,
    ) -> Self {
        Self {
            arguments: arguments.into_iter().collect(),
            initial_commands: initial_commands.into_iter().collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(u64);

impl Display for SessionId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "session-{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProgramHandle(u64);

impl Display for ProgramHandle {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "handle-{}", self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramInit {
    pub protocol_version: ProtocolVersion,
    pub session_id: SessionId,
    pub granted_capabilities: CapabilitySet,
    pub context: ProgramInitContext,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramPhase {
    Init,
    Start,
    Event,
    Command,
    Tick,
    Frame,
}

impl Display for ProgramPhase {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Init => "init",
            Self::Start => "start",
            Self::Event => "event",
            Self::Command => "command",
            Self::Tick => "tick",
            Self::Frame => "frame",
        };
        formatter.write_str(name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramFault {
    pub phase: ProgramPhase,
    pub message: String,
}

impl ProgramFault {
    pub fn new(phase: ProgramPhase, message: impl Into<String>) -> Self {
        Self {
            phase,
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionCloseReason {
    ClientRequested,
    ProgramFault(ProgramFault),
    HostShutdown,
    HostOverloaded,
    ProtocolViolation,
    TransportDisconnected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionState {
    AwaitingInitComplete,
    AwaitingStarted,
    Running,
    Faulted(ProgramFault),
    Closed(SessionCloseReason),
}

impl SessionState {
    const fn is_active(&self) -> bool {
        matches!(
            self,
            Self::AwaitingInitComplete | Self::AwaitingStarted | Self::Running
        )
    }
}

impl Display for SessionState {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::AwaitingInitComplete => "awaiting init completion",
            Self::AwaitingStarted => "awaiting start completion",
            Self::Running => "running",
            Self::Faulted(_) => "faulted",
            Self::Closed(_) => "closed",
        };
        formatter.write_str(name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientMessage {
    InitComplete,
    Started,
    Fault(ProgramFault),
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientMessageKind {
    InitComplete,
    Started,
    Fault,
    Close,
}

impl ClientMessage {
    pub const fn kind(&self) -> ClientMessageKind {
        match self {
            Self::InitComplete => ClientMessageKind::InitComplete,
            Self::Started => ClientMessageKind::Started,
            Self::Fault(_) => ClientMessageKind::Fault,
            Self::Close => ClientMessageKind::Close,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostMessage {
    Init(ProgramInit),
    Start {
        session_id: SessionId,
    },
    Closed {
        session_id: SessionId,
        reason: SessionCloseReason,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    UnsupportedVersion {
        received: ProtocolVersion,
    },
    SessionIdExhausted,
    HandleIdExhausted,
    UnknownSession(SessionId),
    ActiveSession(SessionId),
    InactiveSession(SessionId),
    UnexpectedMessage {
        session_id: SessionId,
        state: SessionState,
        message: ClientMessageKind,
    },
    HandleNotOwned {
        session_id: SessionId,
        handle: ProgramHandle,
    },
}

impl Display for ProtocolError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion { received } => write!(
                formatter,
                "CLP version {received} is not supported; expected {CLP_PROTOCOL_VERSION}"
            ),
            Self::SessionIdExhausted => {
                formatter.write_str("could not allocate another session ID")
            }
            Self::HandleIdExhausted => {
                formatter.write_str("could not allocate another program handle")
            }
            Self::UnknownSession(session_id) => write!(formatter, "unknown {session_id}"),
            Self::ActiveSession(session_id) => write!(formatter, "{session_id} is still active"),
            Self::InactiveSession(session_id) => write!(formatter, "{session_id} is not active"),
            Self::UnexpectedMessage {
                session_id,
                state,
                message,
            } => write!(
                formatter,
                "{message:?} is not valid for {session_id} while {state}"
            ),
            Self::HandleNotOwned { session_id, handle } => {
                write!(formatter, "{handle} is not owned by {session_id}")
            }
        }
    }
}

impl Error for ProtocolError {}

#[derive(Debug)]
struct Session {
    capabilities: CapabilitySet,
    state: SessionState,
    handles: BTreeSet<ProgramHandle>,
}

#[derive(Debug)]
pub struct SessionManager {
    available_capabilities: CapabilitySet,
    next_session_id: u64,
    next_handle_id: u64,
    sessions: BTreeMap<SessionId, Session>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new(CapabilitySet::ALL)
    }
}

impl SessionManager {
    pub fn new(available_capabilities: CapabilitySet) -> Self {
        Self {
            available_capabilities,
            next_session_id: 1,
            next_handle_id: 1,
            sessions: BTreeMap::new(),
        }
    }

    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    pub fn session_state(&self, session_id: SessionId) -> Result<&SessionState, ProtocolError> {
        Ok(&self.session(session_id)?.state)
    }

    pub fn session_capabilities(
        &self,
        session_id: SessionId,
    ) -> Result<CapabilitySet, ProtocolError> {
        Ok(self.session(session_id)?.capabilities)
    }

    pub fn handle_count(&self, session_id: SessionId) -> Result<usize, ProtocolError> {
        Ok(self.session(session_id)?.handles.len())
    }

    pub fn accept(
        &mut self,
        hello: ClientHello,
        context: ProgramInitContext,
    ) -> Result<(SessionId, HostMessage), ProtocolError> {
        if hello.protocol_version.major() != CLP_PROTOCOL_VERSION.major()
            || hello.protocol_version.minor() > CLP_PROTOCOL_VERSION.minor()
        {
            return Err(ProtocolError::UnsupportedVersion {
                received: hello.protocol_version,
            });
        }

        let session_id = self.allocate_session_id()?;
        let capabilities = hello
            .requested_capabilities
            .intersection(self.available_capabilities);
        self.sessions.insert(
            session_id,
            Session {
                capabilities,
                state: SessionState::AwaitingInitComplete,
                handles: BTreeSet::new(),
            },
        );
        Ok((
            session_id,
            HostMessage::Init(ProgramInit {
                protocol_version: hello.protocol_version,
                session_id,
                granted_capabilities: capabilities,
                context,
            }),
        ))
    }

    pub fn receive(
        &mut self,
        session_id: SessionId,
        message: ClientMessage,
    ) -> Result<Option<HostMessage>, ProtocolError> {
        let session = self.session_mut(session_id)?;
        match message {
            ClientMessage::InitComplete => {
                if !matches!(session.state, SessionState::AwaitingInitComplete) {
                    return Err(unexpected_message(
                        session_id,
                        session,
                        ClientMessageKind::InitComplete,
                    ));
                }
                session.state = SessionState::AwaitingStarted;
                Ok(Some(HostMessage::Start { session_id }))
            }
            ClientMessage::Started => {
                if !matches!(session.state, SessionState::AwaitingStarted) {
                    return Err(unexpected_message(
                        session_id,
                        session,
                        ClientMessageKind::Started,
                    ));
                }
                session.state = SessionState::Running;
                Ok(None)
            }
            ClientMessage::Fault(fault) => {
                if !session.state.is_active() {
                    return Err(ProtocolError::InactiveSession(session_id));
                }
                session.handles.clear();
                session.state = SessionState::Faulted(fault.clone());
                Ok(Some(HostMessage::Closed {
                    session_id,
                    reason: SessionCloseReason::ProgramFault(fault),
                }))
            }
            ClientMessage::Close => self
                .close_active_session(session_id, SessionCloseReason::ClientRequested)
                .map(Some),
        }
    }

    pub fn close(
        &mut self,
        session_id: SessionId,
        reason: SessionCloseReason,
    ) -> Result<HostMessage, ProtocolError> {
        self.close_active_session(session_id, reason)
    }

    pub fn remove_terminated(
        &mut self,
        session_id: SessionId,
    ) -> Result<SessionState, ProtocolError> {
        let state = self.session(session_id)?.state.clone();
        if state.is_active() {
            return Err(ProtocolError::ActiveSession(session_id));
        }
        self.sessions.remove(&session_id);
        Ok(state)
    }

    pub fn reserve_handle(
        &mut self,
        session_id: SessionId,
    ) -> Result<ProgramHandle, ProtocolError> {
        if !matches!(self.session(session_id)?.state, SessionState::Running) {
            return Err(ProtocolError::InactiveSession(session_id));
        }
        let handle = ProgramHandle(self.next_handle_id);
        self.next_handle_id = self
            .next_handle_id
            .checked_add(1)
            .ok_or(ProtocolError::HandleIdExhausted)?;
        let session = self.session_mut(session_id)?;
        session.handles.insert(handle);
        Ok(handle)
    }

    pub fn owns_handle(
        &self,
        session_id: SessionId,
        handle: ProgramHandle,
    ) -> Result<bool, ProtocolError> {
        Ok(self.session(session_id)?.handles.contains(&handle))
    }

    pub fn release_handle(
        &mut self,
        session_id: SessionId,
        handle: ProgramHandle,
    ) -> Result<(), ProtocolError> {
        let session = self.session_mut(session_id)?;
        if !session.handles.remove(&handle) {
            return Err(ProtocolError::HandleNotOwned { session_id, handle });
        }
        Ok(())
    }

    fn close_active_session(
        &mut self,
        session_id: SessionId,
        reason: SessionCloseReason,
    ) -> Result<HostMessage, ProtocolError> {
        let session = self.session_mut(session_id)?;
        if !session.state.is_active() {
            return Err(ProtocolError::InactiveSession(session_id));
        }
        session.handles.clear();
        session.state = SessionState::Closed(reason.clone());
        Ok(HostMessage::Closed { session_id, reason })
    }

    fn allocate_session_id(&mut self) -> Result<SessionId, ProtocolError> {
        let session_id = SessionId(self.next_session_id);
        self.next_session_id = self
            .next_session_id
            .checked_add(1)
            .ok_or(ProtocolError::SessionIdExhausted)?;
        Ok(session_id)
    }

    fn session(&self, session_id: SessionId) -> Result<&Session, ProtocolError> {
        self.sessions
            .get(&session_id)
            .ok_or(ProtocolError::UnknownSession(session_id))
    }

    fn session_mut(&mut self, session_id: SessionId) -> Result<&mut Session, ProtocolError> {
        self.sessions
            .get_mut(&session_id)
            .ok_or(ProtocolError::UnknownSession(session_id))
    }
}

fn unexpected_message(
    session_id: SessionId,
    session: &Session,
    message: ClientMessageKind,
) -> ProtocolError {
    ProtocolError::UnexpectedMessage {
        session_id,
        state: session.state.clone(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello(capabilities: CapabilitySet) -> ClientHello {
        ClientHello {
            protocol_version: CLP_PROTOCOL_VERSION,
            requested_capabilities: capabilities,
        }
    }

    fn running_session(manager: &mut SessionManager) -> SessionId {
        let (session_id, _) = manager
            .accept(hello(CapabilitySet::ALL), ProgramInitContext::default())
            .unwrap();
        manager
            .receive(session_id, ClientMessage::InitComplete)
            .unwrap();
        manager.receive(session_id, ClientMessage::Started).unwrap();
        session_id
    }

    #[test]
    fn accepts_a_client_and_preserves_its_init_context() {
        let mut manager = SessionManager::new(CapabilitySet::from_capabilities(&[
            Capability::Scene,
            Capability::Input,
        ]));
        let context = ProgramInitContext::new(
            ["--safe".to_owned()],
            [StartupCommand::new("open", ["welcome".to_owned()])],
        );

        let (session_id, message) = manager
            .accept(hello(CapabilitySet::ALL), context.clone())
            .unwrap();

        assert_eq!(
            message,
            HostMessage::Init(ProgramInit {
                protocol_version: CLP_PROTOCOL_VERSION,
                session_id,
                granted_capabilities: CapabilitySet::from_capabilities(&[
                    Capability::Scene,
                    Capability::Input,
                ]),
                context,
            })
        );
        assert_eq!(
            manager.session_state(session_id),
            Ok(&SessionState::AwaitingInitComplete)
        );
        assert!(
            manager
                .session_capabilities(session_id)
                .unwrap()
                .contains(Capability::Scene)
        );
        assert_eq!(manager.session_count(), 1);
    }

    #[test]
    fn rejects_legacy_major_versions_without_creating_a_session() {
        let mut manager = SessionManager::default();
        let version = ProtocolVersion::new(1, 2);
        let error = manager
            .accept(
                ClientHello {
                    protocol_version: version,
                    requested_capabilities: CapabilitySet::ALL,
                },
                ProgramInitContext::default(),
            )
            .unwrap_err();

        assert_eq!(
            error,
            ProtocolError::UnsupportedVersion { received: version }
        );
        assert_eq!(manager.session_count(), 0);
    }

    #[test]
    fn negotiates_v2_capability_intersection() {
        let mut manager = SessionManager::default();
        let (session_id, message) = manager
            .accept(
                ClientHello {
                    protocol_version: CLP_PROTOCOL_VERSION,
                    requested_capabilities: CapabilitySet::from_capabilities(&[
                        Capability::Scene,
                        Capability::Images,
                    ]),
                },
                ProgramInitContext::default(),
            )
            .unwrap();

        assert_eq!(
            message,
            HostMessage::Init(ProgramInit {
                protocol_version: CLP_PROTOCOL_VERSION,
                session_id,
                granted_capabilities: CapabilitySet::from_capabilities(&[
                    Capability::Scene,
                    Capability::Images,
                ]),
                context: ProgramInitContext::default(),
            })
        );
    }

    #[test]
    fn rejects_an_unsupported_protocol_version_without_creating_a_session() {
        let mut manager = SessionManager::default();

        let error = manager
            .accept(
                ClientHello {
                    protocol_version: ProtocolVersion::new(3, 0),
                    requested_capabilities: CapabilitySet::ALL,
                },
                ProgramInitContext::default(),
            )
            .unwrap_err();

        assert_eq!(
            error,
            ProtocolError::UnsupportedVersion {
                received: ProtocolVersion::new(3, 0),
            }
        );
        assert_eq!(manager.session_count(), 0);
    }

    #[test]
    fn requires_init_completion_before_starting_a_session() {
        let mut manager = SessionManager::default();
        let (session_id, _) = manager
            .accept(hello(CapabilitySet::ALL), ProgramInitContext::default())
            .unwrap();

        assert_eq!(
            manager.receive(session_id, ClientMessage::Started),
            Err(ProtocolError::UnexpectedMessage {
                session_id,
                state: SessionState::AwaitingInitComplete,
                message: ClientMessageKind::Started,
            })
        );
        assert_eq!(
            manager.receive(session_id, ClientMessage::InitComplete),
            Ok(Some(HostMessage::Start { session_id }))
        );
        assert_eq!(
            manager.session_state(session_id),
            Ok(&SessionState::AwaitingStarted)
        );
        assert_eq!(
            manager.receive(session_id, ClientMessage::Started),
            Ok(None)
        );
        assert_eq!(
            manager.session_state(session_id),
            Ok(&SessionState::Running)
        );
    }

    #[test]
    fn does_not_allocate_handles_before_a_session_has_started() {
        let mut manager = SessionManager::default();
        let (session_id, _) = manager
            .accept(hello(CapabilitySet::ALL), ProgramInitContext::default())
            .unwrap();

        assert_eq!(
            manager.reserve_handle(session_id),
            Err(ProtocolError::InactiveSession(session_id))
        );
        assert_eq!(manager.handle_count(session_id), Ok(0));
    }

    #[test]
    fn supports_concurrent_sessions_with_independent_capabilities() {
        let mut manager = SessionManager::default();
        let (first, _) = manager
            .accept(
                hello(CapabilitySet::from_capabilities(&[Capability::Scene])),
                ProgramInitContext::default(),
            )
            .unwrap();
        let (second, _) = manager
            .accept(
                hello(CapabilitySet::from_capabilities(&[Capability::Timing])),
                ProgramInitContext::default(),
            )
            .unwrap();

        assert_ne!(first, second);
        assert_eq!(manager.session_count(), 2);
        assert_eq!(
            manager.session_capabilities(first),
            Ok(CapabilitySet::from_capabilities(&[Capability::Scene]))
        );
        assert_eq!(
            manager.session_capabilities(second),
            Ok(CapabilitySet::from_capabilities(&[Capability::Timing]))
        );
    }

    #[test]
    fn faults_release_only_the_faulted_sessions_handles() {
        let mut manager = SessionManager::default();
        let first = running_session(&mut manager);
        let second = running_session(&mut manager);
        let first_handle = manager.reserve_handle(first).unwrap();
        let second_handle = manager.reserve_handle(second).unwrap();
        let fault = ProgramFault::new(ProgramPhase::Frame, "render callback failed");

        let message = manager
            .receive(first, ClientMessage::Fault(fault.clone()))
            .unwrap();

        assert_eq!(
            message,
            Some(HostMessage::Closed {
                session_id: first,
                reason: SessionCloseReason::ProgramFault(fault.clone()),
            })
        );
        assert_eq!(manager.handle_count(first), Ok(0));
        assert_eq!(
            manager.session_state(first),
            Ok(&SessionState::Faulted(fault))
        );
        assert_eq!(manager.handle_count(second), Ok(1));
        assert!(!manager.owns_handle(second, first_handle).unwrap());
        assert!(manager.owns_handle(second, second_handle).unwrap());
        assert_eq!(manager.session_state(second), Ok(&SessionState::Running));
    }

    #[test]
    fn rejects_handles_from_another_session() {
        let mut manager = SessionManager::default();
        let first = running_session(&mut manager);
        let second = running_session(&mut manager);
        let handle = manager.reserve_handle(first).unwrap();

        assert_eq!(manager.owns_handle(second, handle), Ok(false));
        assert_eq!(
            manager.release_handle(second, handle),
            Err(ProtocolError::HandleNotOwned {
                session_id: second,
                handle,
            })
        );
        assert!(manager.owns_handle(first, handle).unwrap());
    }

    #[test]
    fn closing_a_session_releases_its_handles_without_stopping_others() {
        let mut manager = SessionManager::default();
        let first = running_session(&mut manager);
        let second = running_session(&mut manager);
        manager.reserve_handle(first).unwrap();
        let second_handle = manager.reserve_handle(second).unwrap();

        let message = manager
            .close(first, SessionCloseReason::HostShutdown)
            .unwrap();

        assert_eq!(
            message,
            HostMessage::Closed {
                session_id: first,
                reason: SessionCloseReason::HostShutdown,
            }
        );
        assert_eq!(manager.handle_count(first), Ok(0));
        assert_eq!(
            manager.session_state(first),
            Ok(&SessionState::Closed(SessionCloseReason::HostShutdown))
        );
        assert_eq!(manager.session_state(second), Ok(&SessionState::Running));
        assert!(manager.owns_handle(second, second_handle).unwrap());
    }

    #[test]
    fn client_close_transitions_its_session_to_closed() {
        let mut manager = SessionManager::default();
        let session_id = running_session(&mut manager);

        assert_eq!(
            manager.receive(session_id, ClientMessage::Close),
            Ok(Some(HostMessage::Closed {
                session_id,
                reason: SessionCloseReason::ClientRequested,
            }))
        );
        assert_eq!(
            manager.session_state(session_id),
            Ok(&SessionState::Closed(SessionCloseReason::ClientRequested))
        );
    }

    #[test]
    fn removes_terminated_sessions_without_affecting_active_ones() {
        let mut manager = SessionManager::default();
        let terminated = running_session(&mut manager);
        let active = running_session(&mut manager);
        manager.receive(terminated, ClientMessage::Close).unwrap();

        assert_eq!(
            manager.remove_terminated(terminated),
            Ok(SessionState::Closed(SessionCloseReason::ClientRequested))
        );
        assert_eq!(manager.session_count(), 1);
        assert_eq!(
            manager.session_state(terminated),
            Err(ProtocolError::UnknownSession(terminated))
        );
        assert_eq!(manager.session_state(active), Ok(&SessionState::Running));
        assert_eq!(
            manager.remove_terminated(active),
            Err(ProtocolError::ActiveSession(active))
        );
    }
}

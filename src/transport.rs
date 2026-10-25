//! Local Chymeral Link Protocol (CLP) server and session lifecycle.
//!
//! This facade exposes endpoint configuration, host actions, presentation
//! submissions, and notification metrics. The private `server` module owns
//! endpoint setup, handshake and capability negotiation, session lifecycle,
//! and local-socket I/O; `framing` preserves the length-prefix and
//! nonblocking-I/O invariants shared by JSON and binary CLP messages.
//!
//! Host actions, response ordering, bounded notification queues, and session
//! cleanup form the host-side concurrency contract observed by the executable,
//! not a public Rust API.

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
mod server;

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
use server::{PresentationMailbox, ServerControl};

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

// The wire schema remains in the transport module's privacy boundary so the
// test corpus can exercise private malformed-frame cases without widening its
// visibility. It is kept in a separate source file for navigation.
include!("transport/wire.rs");

#[cfg(test)]
use server::{
    FrameRead, FrameReader, RunningFrameWriter, command_response_message, resize_notification,
    wire_command_is_granted,
};

#[cfg(test)]
mod tests {
    include!("transport/tests.rs");
}

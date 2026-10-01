use std::{
    collections::VecDeque,
    error::Error,
    fmt::{Display, Formatter},
    time::{Duration, Instant},
};

use crate::scene::NodeId;

const MAX_TICKS_PER_FRAME: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeError {
    message: String,
}

impl RuntimeError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for RuntimeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(formatter)
    }
}

impl Error for RuntimeError {}

pub type RuntimeResult<T> = std::result::Result<T, RuntimeError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimePhase {
    Init,
    Start,
    Event,
    Command,
    Tick,
    Frame,
}

impl Display for RuntimePhase {
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
pub struct RuntimeFault {
    pub phase: RuntimePhase,
    pub error: RuntimeError,
}

impl Display for RuntimeFault {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "runtime {} failed: {}", self.phase, self.error)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuntimeMetrics {
    pub event_count: u64,
    pub command_count: u64,
    pub tick_count: u64,
    pub dropped_tick_count: u64,
    pub frame_count: u64,
    pub last_tick_dt: Option<Duration>,
    pub last_frame_dt: Option<Duration>,
    pub last_frame_tick_count: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub super_key: bool,
}

impl Modifiers {
    pub const fn control_key(self) -> bool {
        self.control
    }

    pub const fn alt_key(self) -> bool {
        self.alt
    }

    pub const fn super_key(self) -> bool {
        self.super_key
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyState {
    Pressed,
    Released,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub text: Option<String>,
    pub state: KeyState,
    pub modifiers: Modifiers,
    pub repeat: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Character(String),
    Named(NamedKey),
    Dead(Option<char>),
    Unidentified,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamedKey {
    Alt,
    Backspace,
    Delete,
    End,
    Enter,
    Escape,
    Home,
    Insert,
    PageDown,
    PageUp,
    Space,
    Tab,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Other(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum PointerEvent {
    Move {
        position: Vec2,
        modifiers: Modifiers,
    },
    Press {
        button: PointerButton,
        position: Vec2,
        modifiers: Modifiers,
    },
    Release {
        button: PointerButton,
        position: Vec2,
        modifiers: Modifiers,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Secondary,
    Middle,
    Back,
    Forward,
    Other(u16),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScrollEvent {
    pub position: Vec2,
    pub delta: Vec2,
    pub unit: ScrollUnit,
    pub modifiers: Modifiers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollUnit {
    Line,
    Pixel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusEvent {
    Gained,
    Lost,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SceneEvent {
    PointerEnter {
        target: NodeId,
        position: Vec2,
        modifiers: Modifiers,
    },
    PointerLeave {
        target: NodeId,
        position: Vec2,
        modifiers: Modifiers,
    },
    PointerMove {
        target: NodeId,
        position: Vec2,
        modifiers: Modifiers,
    },
    PointerPress {
        target: NodeId,
        button: PointerButton,
        position: Vec2,
        modifiers: Modifiers,
    },
    PointerRelease {
        target: NodeId,
        button: PointerButton,
        position: Vec2,
        modifiers: Modifiers,
    },
    FocusGained {
        target: NodeId,
    },
    FocusLost {
        target: NodeId,
    },
    DragStart {
        source: NodeId,
        origin: Vec2,
        position: Vec2,
        modifiers: Modifiers,
    },
    DragEnter {
        source: NodeId,
        target: NodeId,
        position: Vec2,
        modifiers: Modifiers,
    },
    DragLeave {
        source: NodeId,
        target: NodeId,
        position: Vec2,
        modifiers: Modifiers,
    },
    DragMove {
        source: NodeId,
        target: Option<NodeId>,
        position: Vec2,
        modifiers: Modifiers,
    },
    Drop {
        source: NodeId,
        target: NodeId,
        position: Vec2,
        modifiers: Modifiers,
    },
    DragEnd {
        source: NodeId,
        target: Option<NodeId>,
        position: Vec2,
        modifiers: Modifiers,
    },
    DragCancel {
        source: NodeId,
        position: Vec2,
        modifiers: Modifiers,
    },
    OverlapEnter {
        first: NodeId,
        second: NodeId,
    },
    OverlapLeave {
        first: NodeId,
        second: NodeId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    OutputAvailable,
    Exited,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomEvent {
    pub name: String,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    Key(KeyEvent),
    Pointer(PointerEvent),
    Scene(SceneEvent),
    Scroll(ScrollEvent),
    Focus(FocusEvent),
    Terminal(TerminalEvent),
    Custom(CustomEvent),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeCommand {
    pub name: String,
    pub arguments: Vec<String>,
}

impl RuntimeCommand {
    pub fn new(name: impl Into<String>, arguments: impl IntoIterator<Item = String>) -> Self {
        Self {
            name: name.into(),
            arguments: arguments.into_iter().collect(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InitContext {
    pub arguments: Vec<String>,
    pub initial_commands: Vec<RuntimeCommand>,
}

impl InitContext {
    pub fn new(
        arguments: impl IntoIterator<Item = String>,
        initial_commands: impl IntoIterator<Item = RuntimeCommand>,
    ) -> Self {
        Self {
            arguments: arguments.into_iter().collect(),
            initial_commands: initial_commands.into_iter().collect(),
        }
    }
}

pub trait Program {
    fn init(&mut self, context: &mut InitContext) -> RuntimeResult<()>;
    fn start(&mut self) -> RuntimeResult<()>;
    fn on_event(&mut self, event: &EngineEvent) -> RuntimeResult<()>;
    fn on_command(&mut self, command: &RuntimeCommand) -> RuntimeResult<()>;
    fn tick(&mut self, dt: Duration) -> RuntimeResult<()>;
    fn frame(&mut self, dt: Duration) -> RuntimeResult<()>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeState {
    Created,
    Running,
    Faulted(RuntimeFault),
}

#[derive(Clone, Debug)]
enum RuntimeMessage {
    Event(EngineEvent),
    Command(RuntimeCommand),
}

pub struct Runtime<P> {
    program: P,
    state: RuntimeState,
    messages: VecDeque<RuntimeMessage>,
    tick_interval: Duration,
    next_tick: Option<Instant>,
    last_frame: Option<Instant>,
    metrics: RuntimeMetrics,
    ticks_since_last_frame: u32,
}

impl<P: Program> Runtime<P> {
    pub fn new(program: P, tick_interval: Duration) -> RuntimeResult<Self> {
        if tick_interval.is_zero() {
            return Err(RuntimeError::new(
                "the runtime tick interval must be greater than zero",
            ));
        }

        Ok(Self {
            program,
            state: RuntimeState::Created,
            messages: VecDeque::new(),
            tick_interval,
            next_tick: None,
            last_frame: None,
            metrics: RuntimeMetrics::default(),
            ticks_since_last_frame: 0,
        })
    }

    pub fn initialize(&mut self, mut context: InitContext, now: Instant) -> RuntimeResult<()> {
        if !matches!(self.state, RuntimeState::Created) {
            return Err(RuntimeError::new(
                "the runtime has already been initialized",
            ));
        }

        if let Err(error) = self.program.init(&mut context) {
            return self.fail(RuntimePhase::Init, error);
        }
        if let Err(error) = self.program.start() {
            return self.fail(RuntimePhase::Start, error);
        }
        self.messages.extend(
            context
                .initial_commands
                .into_iter()
                .map(RuntimeMessage::Command),
        );
        self.next_tick = Some(now + self.tick_interval);
        self.last_frame = Some(now);
        self.state = RuntimeState::Running;
        Ok(())
    }

    pub fn is_started(&self) -> bool {
        matches!(self.state, RuntimeState::Running)
    }

    pub fn is_faulted(&self) -> bool {
        matches!(self.state, RuntimeState::Faulted(_))
    }

    pub fn state(&self) -> &RuntimeState {
        &self.state
    }

    pub fn fault(&self) -> Option<&RuntimeFault> {
        let RuntimeState::Faulted(fault) = &self.state else {
            return None;
        };
        Some(fault)
    }

    pub fn enqueue_event(&mut self, event: EngineEvent) -> bool {
        if !self.is_started() {
            return false;
        }
        self.messages.push_back(RuntimeMessage::Event(event));
        true
    }

    pub fn enqueue_command(&mut self, command: RuntimeCommand) -> bool {
        if !self.is_started() {
            return false;
        }
        self.messages.push_back(RuntimeMessage::Command(command));
        true
    }

    pub fn run_frame(&mut self, now: Instant) -> RuntimeResult<()> {
        if !self.ensure_running()? {
            return Ok(());
        }

        self.run_update(now)?;
        let last_frame = self.last_frame.replace(now).unwrap_or(now);
        let frame_dt = now.saturating_duration_since(last_frame);
        if let Err(error) = self.program.frame(frame_dt) {
            return self.fail(RuntimePhase::Frame, error);
        }
        self.metrics.frame_count += 1;
        self.metrics.last_frame_dt = Some(frame_dt);
        self.metrics.last_frame_tick_count = self.ticks_since_last_frame;
        self.ticks_since_last_frame = 0;
        Ok(())
    }

    pub fn run_update(&mut self, now: Instant) -> RuntimeResult<()> {
        if !self.ensure_running()? {
            return Ok(());
        }

        self.dispatch_messages()?;
        self.run_ticks(now)
    }

    pub fn next_tick_deadline(&self) -> Option<Instant> {
        self.is_started().then_some(self.next_tick).flatten()
    }

    pub fn metrics(&self) -> RuntimeMetrics {
        self.metrics
    }

    pub fn program(&self) -> &P {
        &self.program
    }

    pub fn program_mut(&mut self) -> &mut P {
        &mut self.program
    }

    fn dispatch_messages(&mut self) -> RuntimeResult<()> {
        while let Some(message) = self.messages.pop_front() {
            let (phase, result) = match message {
                RuntimeMessage::Event(event) => {
                    (RuntimePhase::Event, self.program.on_event(&event))
                }
                RuntimeMessage::Command(command) => {
                    (RuntimePhase::Command, self.program.on_command(&command))
                }
            };
            if let Err(error) = result {
                return self.fail(phase, error);
            }
            match phase {
                RuntimePhase::Event => self.metrics.event_count += 1,
                RuntimePhase::Command => self.metrics.command_count += 1,
                _ => unreachable!(),
            }
        }
        Ok(())
    }

    fn run_ticks(&mut self, now: Instant) -> RuntimeResult<()> {
        let Some(mut next_tick) = self.next_tick else {
            return Ok(());
        };

        let mut ticks = 0;
        while next_tick <= now && ticks < MAX_TICKS_PER_FRAME {
            if let Err(error) = self.program.tick(self.tick_interval) {
                return self.fail(RuntimePhase::Tick, error);
            }
            self.metrics.tick_count += 1;
            self.metrics.last_tick_dt = Some(self.tick_interval);
            self.ticks_since_last_frame = self.ticks_since_last_frame.saturating_add(1);
            next_tick += self.tick_interval;
            ticks += 1;
        }
        if next_tick <= now {
            let skipped = now
                .duration_since(next_tick)
                .as_nanos()
                .saturating_div(self.tick_interval.as_nanos())
                .saturating_add(1)
                .min(u128::from(u64::MAX)) as u64;
            self.metrics.dropped_tick_count =
                self.metrics.dropped_tick_count.saturating_add(skipped);
            next_tick = now + self.tick_interval;
        }
        self.next_tick = Some(next_tick);
        Ok(())
    }

    fn ensure_running(&self) -> RuntimeResult<bool> {
        match &self.state {
            RuntimeState::Created => Ok(false),
            RuntimeState::Running => Ok(true),
            RuntimeState::Faulted(fault) => Err(RuntimeError::new(fault.to_string())),
        }
    }

    fn fail<T>(&mut self, phase: RuntimePhase, error: RuntimeError) -> RuntimeResult<T> {
        let fault = RuntimeFault { phase, error };
        let result = Err(RuntimeError::new(fault.to_string()));
        self.messages.clear();
        self.next_tick = None;
        self.last_frame = None;
        self.state = RuntimeState::Faulted(fault);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        calls: Vec<String>,
        fail_phase: Option<RuntimePhase>,
    }

    impl Recorder {
        fn maybe_fail(&self, phase: RuntimePhase) -> RuntimeResult<()> {
            if self.fail_phase == Some(phase) {
                return Err(RuntimeError::new(format!("{phase} callback failed")));
            }
            Ok(())
        }
    }

    impl Program for Recorder {
        fn init(&mut self, context: &mut InitContext) -> RuntimeResult<()> {
            self.calls
                .push(format!("init:{}", context.arguments.join(",")));
            self.maybe_fail(RuntimePhase::Init)
        }

        fn start(&mut self) -> RuntimeResult<()> {
            self.calls.push("start".into());
            self.maybe_fail(RuntimePhase::Start)
        }

        fn on_event(&mut self, event: &EngineEvent) -> RuntimeResult<()> {
            let name = match event {
                EngineEvent::Key(_) => "key",
                EngineEvent::Pointer(_) => "pointer",
                EngineEvent::Scene(_) => "scene",
                EngineEvent::Scroll(_) => "scroll",
                EngineEvent::Focus(_) => "focus",
                EngineEvent::Terminal(_) => "terminal",
                EngineEvent::Custom(event) => event.name.as_str(),
            };
            self.calls.push(format!("event:{name}"));
            self.maybe_fail(RuntimePhase::Event)
        }

        fn on_command(&mut self, command: &RuntimeCommand) -> RuntimeResult<()> {
            self.calls.push(format!("command:{}", command.name));
            self.maybe_fail(RuntimePhase::Command)
        }

        fn tick(&mut self, _: Duration) -> RuntimeResult<()> {
            self.calls.push("tick".into());
            self.maybe_fail(RuntimePhase::Tick)
        }

        fn frame(&mut self, _: Duration) -> RuntimeResult<()> {
            self.calls.push("frame".into());
            self.maybe_fail(RuntimePhase::Frame)
        }
    }

    fn runtime(recorder: Recorder) -> Runtime<Recorder> {
        Runtime::new(recorder, Duration::from_millis(10)).unwrap()
    }

    #[test]
    fn initializes_before_start_and_delivers_initial_commands_afterward() {
        let now = Instant::now();
        let mut runtime = runtime(Recorder::default());
        assert!(!runtime.enqueue_command(RuntimeCommand::new("ignored", [])));

        runtime
            .initialize(
                InitContext::new(["--profile".to_owned()], [RuntimeCommand::new("boot", [])]),
                now,
            )
            .unwrap();
        runtime.run_frame(now).unwrap();

        assert_eq!(
            runtime.program().calls,
            ["init:--profile", "start", "command:boot", "frame"]
        );
        assert_eq!(
            runtime.next_tick_deadline(),
            Some(now + Duration::from_millis(10))
        );
    }

    #[test]
    fn preserves_arrival_order_across_commands_and_events() {
        let now = Instant::now();
        let mut runtime = runtime(Recorder::default());
        runtime.initialize(InitContext::default(), now).unwrap();
        runtime.enqueue_event(EngineEvent::Custom(CustomEvent {
            name: "first".into(),
            payload: vec![],
        }));
        runtime.enqueue_command(RuntimeCommand::new("second", []));
        runtime.enqueue_event(EngineEvent::Custom(CustomEvent {
            name: "third".into(),
            payload: vec![],
        }));

        runtime.run_frame(now).unwrap();

        assert_eq!(
            runtime.program().calls,
            [
                "init:",
                "start",
                "event:first",
                "command:second",
                "event:third",
                "frame",
            ]
        );
    }

    #[test]
    fn dispatches_each_canonical_input_event() {
        let now = Instant::now();
        let mut runtime = runtime(Recorder::default());
        runtime.initialize(InitContext::default(), now).unwrap();
        let node = crate::scene::Scene::new().create_root(crate::scene::NodeProperties::default());
        for event in [
            EngineEvent::Key(KeyEvent {
                key: Key::Character("a".into()),
                text: Some("a".into()),
                state: KeyState::Released,
                modifiers: Modifiers::default(),
                repeat: false,
            }),
            EngineEvent::Pointer(PointerEvent::Move {
                position: Vec2 { x: 12.0, y: 24.0 },
                modifiers: Modifiers::default(),
            }),
            EngineEvent::Scene(SceneEvent::FocusGained { target: node }),
            EngineEvent::Scroll(ScrollEvent {
                position: Vec2 { x: 12.0, y: 24.0 },
                delta: Vec2 { x: 0.0, y: -1.0 },
                unit: ScrollUnit::Line,
                modifiers: Modifiers::default(),
            }),
            EngineEvent::Focus(FocusEvent::Lost),
        ] {
            assert!(runtime.enqueue_event(event));
        }

        runtime.run_update(now).unwrap();

        assert_eq!(
            runtime.program().calls,
            [
                "init:",
                "start",
                "event:key",
                "event:pointer",
                "event:scene",
                "event:scroll",
                "event:focus",
            ]
        );
    }

    #[test]
    fn runs_fixed_ticks_before_each_frame() {
        let now = Instant::now();
        let mut runtime = runtime(Recorder::default());
        runtime.initialize(InitContext::default(), now).unwrap();

        runtime.run_frame(now + Duration::from_millis(30)).unwrap();

        assert_eq!(
            runtime.program().calls,
            ["init:", "start", "tick", "tick", "tick", "frame"]
        );
    }

    #[test]
    fn exposes_metrics_for_dispatched_work_and_frame_timing() {
        let now = Instant::now();
        let mut runtime = runtime(Recorder::default());
        runtime.initialize(InitContext::default(), now).unwrap();
        runtime.enqueue_event(EngineEvent::Custom(CustomEvent {
            name: "event".into(),
            payload: vec![],
        }));
        runtime.enqueue_command(RuntimeCommand::new("command", []));

        runtime.run_frame(now + Duration::from_millis(20)).unwrap();

        assert_eq!(
            runtime.metrics(),
            RuntimeMetrics {
                event_count: 1,
                command_count: 1,
                tick_count: 2,
                dropped_tick_count: 0,
                frame_count: 1,
                last_tick_dt: Some(Duration::from_millis(10)),
                last_frame_dt: Some(Duration::from_millis(20)),
                last_frame_tick_count: 2,
            }
        );
    }

    #[test]
    fn updates_ticks_without_running_a_frame() {
        let now = Instant::now();
        let mut runtime = runtime(Recorder::default());
        runtime.initialize(InitContext::default(), now).unwrap();

        runtime.run_update(now + Duration::from_millis(10)).unwrap();

        assert_eq!(runtime.program().calls, ["init:", "start", "tick"]);
    }

    #[test]
    fn does_not_start_or_deliver_messages_when_initialization_fails() {
        let now = Instant::now();
        let mut runtime = runtime(Recorder {
            calls: vec![],
            fail_phase: Some(RuntimePhase::Init),
        });

        assert!(runtime.initialize(InitContext::default(), now).is_err());
        assert!(!runtime.is_started());
        assert!(runtime.is_faulted());
        assert_eq!(runtime.fault().unwrap().phase, RuntimePhase::Init);
        assert!(!runtime.enqueue_event(EngineEvent::Custom(CustomEvent {
            name: "ignored".into(),
            payload: vec![],
        })));
        assert!(runtime.run_frame(now).is_err());

        assert_eq!(runtime.program().calls, ["init:"]);
    }

    #[test]
    fn does_not_start_when_start_fails() {
        let now = Instant::now();
        let mut runtime = runtime(Recorder {
            calls: vec![],
            fail_phase: Some(RuntimePhase::Start),
        });

        assert!(runtime.initialize(InitContext::default(), now).is_err());
        assert!(!runtime.is_started());
        assert_eq!(runtime.fault().unwrap().phase, RuntimePhase::Start);
        assert_eq!(runtime.program().calls, ["init:", "start"]);
    }

    #[test]
    fn faults_and_stops_after_a_runtime_callback_error() {
        for phase in [
            RuntimePhase::Event,
            RuntimePhase::Command,
            RuntimePhase::Tick,
            RuntimePhase::Frame,
        ] {
            let now = Instant::now();
            let mut runtime = runtime(Recorder {
                calls: vec![],
                fail_phase: Some(phase),
            });
            runtime.initialize(InitContext::default(), now).unwrap();

            match phase {
                RuntimePhase::Event => {
                    runtime.enqueue_event(EngineEvent::Custom(CustomEvent {
                        name: "fail".into(),
                        payload: vec![],
                    }));
                    runtime.enqueue_command(RuntimeCommand::new("discarded", []));
                    assert!(runtime.run_update(now).is_err());
                }
                RuntimePhase::Command => {
                    runtime.enqueue_command(RuntimeCommand::new("fail", []));
                    assert!(runtime.run_update(now).is_err());
                }
                RuntimePhase::Tick => {
                    assert!(runtime.run_update(now + Duration::from_millis(10)).is_err());
                }
                RuntimePhase::Frame => assert!(runtime.run_frame(now).is_err()),
                RuntimePhase::Init | RuntimePhase::Start => unreachable!(),
            }

            assert!(runtime.is_faulted());
            assert_eq!(runtime.fault().unwrap().phase, phase);
            assert_eq!(runtime.next_tick_deadline(), None);
            assert!(!runtime.enqueue_event(EngineEvent::Custom(CustomEvent {
                name: "ignored".into(),
                payload: vec![],
            })));
            let calls = runtime.program().calls.clone();
            assert!(runtime.run_update(now + Duration::from_secs(1)).is_err());
            assert_eq!(runtime.program().calls, calls);
        }
    }

    #[test]
    fn rejects_a_zero_tick_interval() {
        assert!(Runtime::new(Recorder::default(), Duration::ZERO).is_err());
    }
}

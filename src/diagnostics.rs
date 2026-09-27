use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use moditty::runtime::{
    EngineEvent, Key as EngineKey, KeyEvent as EngineKeyEvent, KeyState, Modifiers, RuntimeMetrics,
};

const TRACE_CAPACITY: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub key: char,
    pub modifiers: Modifiers,
}

impl Shortcut {
    pub fn parse(value: &str) -> Result<Self, String> {
        let mut modifiers = Modifiers::default();
        let mut key = None;

        for part in value.split('+').map(str::trim) {
            let normalized = part.to_ascii_lowercase();
            match normalized.as_str() {
                "ctrl" | "control" if !modifiers.control => modifiers.control = true,
                "shift" if !modifiers.shift => modifiers.shift = true,
                "alt" if !modifiers.alt => modifiers.alt = true,
                "super" | "meta" if !modifiers.super_key => modifiers.super_key = true,
                "ctrl" | "control" | "shift" | "alt" | "super" | "meta" => {
                    return Err(format!("duplicate modifier in shortcut {value:?}"));
                }
                _ if part.len() == 1
                    && part.is_ascii()
                    && part.as_bytes()[0].is_ascii_alphabetic() =>
                {
                    if key
                        .replace((part.as_bytes()[0] as char).to_ascii_uppercase())
                        .is_some()
                    {
                        return Err(format!("shortcut {value:?} must contain exactly one key"));
                    }
                }
                _ => return Err(format!("invalid shortcut component {part:?}")),
            }
        }

        let Some(key) = key else {
            return Err(format!("shortcut {value:?} must contain a key"));
        };
        if !(modifiers.control || modifiers.shift || modifiers.alt || modifiers.super_key) {
            return Err(format!("shortcut {value:?} must contain a modifier"));
        }

        Ok(Self { key, modifiers })
    }

    pub fn matches(&self, event: &EngineKeyEvent) -> bool {
        self.modifiers == event.modifiers
            && matches!(
                &event.key,
                EngineKey::Character(character)
                    if character.len() == 1 && character.eq_ignore_ascii_case(&self.key.to_string())
            )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticsShortcuts {
    pub events: Shortcut,
    pub ticks: Shortcut,
    pub frames: Shortcut,
}

impl Default for DiagnosticsShortcuts {
    fn default() -> Self {
        Self {
            events: Shortcut {
                key: 'E',
                modifiers: Modifiers {
                    control: true,
                    shift: true,
                    ..Modifiers::default()
                },
            },
            ticks: Shortcut {
                key: 'T',
                modifiers: Modifiers {
                    control: true,
                    shift: true,
                    ..Modifiers::default()
                },
            },
            frames: Shortcut {
                key: 'F',
                modifiers: Modifiers {
                    control: true,
                    shift: true,
                    ..Modifiers::default()
                },
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticView {
    Events,
    Ticks,
    Frames,
}

impl DiagnosticView {
    fn title(self) -> &'static str {
        match self {
            Self::Events => "EVENTS",
            Self::Ticks => "TICKS",
            Self::Frames => "FRAMES",
        }
    }
}

#[derive(Clone, Debug)]
struct TraceEntry {
    elapsed: Duration,
    text: String,
}

pub struct Diagnostics {
    shortcuts: DiagnosticsShortcuts,
    active: Option<DiagnosticView>,
    started_at: Instant,
    entries: VecDeque<TraceEntry>,
    observed_metrics: RuntimeMetrics,
}

impl Diagnostics {
    pub fn new(shortcuts: DiagnosticsShortcuts, now: Instant, metrics: RuntimeMetrics) -> Self {
        Self {
            shortcuts,
            active: None,
            started_at: now,
            entries: VecDeque::with_capacity(TRACE_CAPACITY),
            observed_metrics: metrics,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub fn needs_continuous_redraw(&self) -> bool {
        matches!(
            self.active,
            Some(DiagnosticView::Ticks | DiagnosticView::Frames)
        )
    }

    pub fn handle_key(
        &mut self,
        event: &EngineKeyEvent,
        now: Instant,
        metrics: RuntimeMetrics,
    ) -> bool {
        if self.active.is_some()
            && matches!(
                event.key,
                EngineKey::Named(moditty::runtime::NamedKey::Escape)
            )
            && event.state == KeyState::Pressed
        {
            self.active = None;
            self.entries.clear();
            return true;
        }

        let view = [
            (DiagnosticView::Events, &self.shortcuts.events),
            (DiagnosticView::Ticks, &self.shortcuts.ticks),
            (DiagnosticView::Frames, &self.shortcuts.frames),
        ]
        .into_iter()
        .find_map(|(view, shortcut)| shortcut.matches(event).then_some(view));
        let Some(view) = view else {
            return false;
        };

        if event.state == KeyState::Pressed && !event.repeat {
            self.active = Some(view);
            self.started_at = now;
            self.entries.clear();
            self.observed_metrics = metrics;
        }
        true
    }

    pub fn record_event(&mut self, event: &EngineEvent, now: Instant) {
        if self.active != Some(DiagnosticView::Events) {
            return;
        }
        self.push(now, describe_event(event));
    }

    pub fn record_metrics(&mut self, metrics: RuntimeMetrics, now: Instant) {
        let tick_start = self.observed_metrics.tick_count;
        let frame_start = self.observed_metrics.frame_count;
        match self.active {
            Some(DiagnosticView::Ticks) if metrics.tick_count > tick_start => {
                for tick in tick_start + 1..=metrics.tick_count {
                    self.push(
                        now,
                        format!(
                            "tick #{tick} dt={} total={} dropped={}",
                            duration_text(metrics.last_tick_dt),
                            metrics.tick_count,
                            metrics.dropped_tick_count,
                        ),
                    );
                }
            }
            Some(DiagnosticView::Frames) if metrics.frame_count > frame_start => {
                for frame in frame_start + 1..=metrics.frame_count {
                    self.push(
                        now,
                        format!(
                            "frame #{frame} dt={} ticks={}",
                            duration_text(metrics.last_frame_dt),
                            metrics.last_frame_tick_count,
                        ),
                    );
                }
            }
            _ => {}
        }
        self.observed_metrics = metrics;
    }

    pub fn render_text(&self, metrics: RuntimeMetrics, rows: usize) -> Option<String> {
        let view = self.active?;
        let metrics_line = match view {
            DiagnosticView::Events => format!(
                "events={} commands={}",
                metrics.event_count, metrics.command_count
            ),
            DiagnosticView::Ticks => format!(
                "ticks={} last_dt={} dropped_ticks={}",
                metrics.tick_count,
                duration_text(metrics.last_tick_dt),
                metrics.dropped_tick_count,
            ),
            DiagnosticView::Frames => format!(
                "frames={} last_dt={} ticks_in_frame={}",
                metrics.frame_count,
                duration_text(metrics.last_frame_dt),
                metrics.last_frame_tick_count,
            ),
        };
        let mut lines = vec![
            format!("MODITTY DIAGNOSTICS — {}", view.title()),
            "Esc: return to terminal | terminal input is paused".to_owned(),
            metrics_line,
            String::new(),
        ];
        let entry_limit = rows.saturating_sub(lines.len()).max(1);
        let start = self.entries.len().saturating_sub(entry_limit);
        lines.extend(
            self.entries
                .iter()
                .skip(start)
                .map(|entry| format!("{:>8.3}s {}", entry.elapsed.as_secs_f64(), entry.text)),
        );
        Some(lines.join("\n"))
    }

    fn push(&mut self, now: Instant, text: String) {
        if self.entries.len() == TRACE_CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(TraceEntry {
            elapsed: now.saturating_duration_since(self.started_at),
            text,
        });
    }
}

fn duration_text(duration: Option<Duration>) -> String {
    duration
        .map(|duration| format!("{:.3}ms", duration.as_secs_f64() * 1_000.0))
        .unwrap_or_else(|| "n/a".to_owned())
}

fn describe_event(event: &EngineEvent) -> String {
    match event {
        EngineEvent::Key(event) => {
            let action = match event.state {
                KeyState::Pressed => "press",
                KeyState::Released => "release",
            };
            let key = match &event.key {
                EngineKey::Character(_) => "character=<redacted>".to_owned(),
                EngineKey::Named(key) => format!("named={key:?}"),
                EngineKey::Dead(_) => "dead-key=<redacted>".to_owned(),
                EngineKey::Unidentified => "unidentified".to_owned(),
            };
            format!("key {action} {key}{}", modifier_text(event.modifiers))
        }
        EngineEvent::Pointer(event) => format!("pointer {event:?}"),
        EngineEvent::Scene(event) => format!("scene {event:?}"),
        EngineEvent::Scroll(event) => format!(
            "scroll unit={:?} delta=({:.1}, {:.1}) position=({:.1}, {:.1}){}",
            event.unit,
            event.delta.x,
            event.delta.y,
            event.position.x,
            event.position.y,
            modifier_text(event.modifiers),
        ),
        EngineEvent::Focus(event) => format!("focus {event:?}"),
        EngineEvent::Terminal(event) => format!("terminal {event:?}"),
        EngineEvent::Custom(_) => "custom event=<redacted>".to_owned(),
    }
}

fn modifier_text(modifiers: Modifiers) -> String {
    let mut names = Vec::new();
    if modifiers.control {
        names.push("Ctrl");
    }
    if modifiers.shift {
        names.push("Shift");
    }
    if modifiers.alt {
        names.push("Alt");
    }
    if modifiers.super_key {
        names.push("Super");
    }
    if names.is_empty() {
        String::new()
    } else {
        format!(" modifiers={}", names.join("+"))
    }
}

#[cfg(test)]
mod tests {
    use moditty::{
        runtime::SceneEvent,
        scene::{NodeProperties, Scene},
    };

    use super::*;

    fn key_event(key: char, modifiers: Modifiers) -> EngineKeyEvent {
        EngineKeyEvent {
            key: EngineKey::Character(key.to_string()),
            text: Some(key.to_string()),
            state: KeyState::Pressed,
            modifiers,
            repeat: false,
        }
    }

    #[test]
    fn parses_and_matches_a_modified_shortcut() {
        let shortcut = Shortcut::parse("Ctrl+Shift+E").unwrap();
        assert!(shortcut.matches(&key_event(
            'e',
            Modifiers {
                control: true,
                shift: true,
                ..Modifiers::default()
            },
        )));
        assert!(!Shortcut::parse("E").is_ok());
        assert!(!Shortcut::parse("Ctrl+Shift+Enter").is_ok());
    }

    #[test]
    fn redacts_printable_input_from_event_traces() {
        let now = Instant::now();
        let mut diagnostics = Diagnostics::new(
            DiagnosticsShortcuts::default(),
            now,
            RuntimeMetrics::default(),
        );
        diagnostics.active = Some(DiagnosticView::Events);
        diagnostics.record_event(
            &EngineEvent::Key(EngineKeyEvent {
                key: EngineKey::Character("top-secret".into()),
                text: Some("top-secret".into()),
                state: KeyState::Pressed,
                modifiers: Modifiers::default(),
                repeat: false,
            }),
            now,
        );

        let text = diagnostics
            .render_text(RuntimeMetrics::default(), 10)
            .unwrap();
        assert!(text.contains("<redacted>"));
        assert!(!text.contains("top-secret"));
    }

    #[test]
    fn records_targeted_scene_events() {
        let now = Instant::now();
        let mut diagnostics = Diagnostics::new(
            DiagnosticsShortcuts::default(),
            now,
            RuntimeMetrics::default(),
        );
        diagnostics.active = Some(DiagnosticView::Events);
        let target = Scene::new().create_root(NodeProperties::default());
        diagnostics.record_event(&EngineEvent::Scene(SceneEvent::FocusGained { target }), now);

        let text = diagnostics
            .render_text(RuntimeMetrics::default(), 10)
            .unwrap();
        assert!(text.contains("scene FocusGained"));
    }

    #[test]
    fn records_only_the_active_metric_view() {
        let now = Instant::now();
        let metrics = RuntimeMetrics::default();
        let mut diagnostics = Diagnostics::new(DiagnosticsShortcuts::default(), now, metrics);
        diagnostics.active = Some(DiagnosticView::Ticks);
        diagnostics.record_metrics(
            RuntimeMetrics {
                tick_count: 2,
                last_tick_dt: Some(Duration::from_millis(16)),
                ..RuntimeMetrics::default()
            },
            now,
        );

        let text = diagnostics
            .render_text(
                RuntimeMetrics {
                    tick_count: 2,
                    ..RuntimeMetrics::default()
                },
                10,
            )
            .unwrap();
        assert!(text.contains("tick #1"));
        assert!(text.contains("tick #2"));
    }

    #[test]
    fn opens_with_its_shortcut_and_closes_with_escape() {
        let now = Instant::now();
        let mut diagnostics = Diagnostics::new(
            DiagnosticsShortcuts::default(),
            now,
            RuntimeMetrics::default(),
        );
        let shortcut = key_event(
            'e',
            Modifiers {
                control: true,
                shift: true,
                ..Modifiers::default()
            },
        );
        assert!(diagnostics.handle_key(&shortcut, now, RuntimeMetrics::default()));
        assert!(diagnostics.is_active());

        let escape = EngineKeyEvent {
            key: EngineKey::Named(moditty::runtime::NamedKey::Escape),
            text: None,
            state: KeyState::Pressed,
            modifiers: Modifiers::default(),
            repeat: false,
        };
        assert!(diagnostics.handle_key(&escape, now, RuntimeMetrics::default()));
        assert!(!diagnostics.is_active());
    }
}

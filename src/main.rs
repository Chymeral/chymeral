#[cfg(not(any(target_os = "linux", target_os = "windows")))]
compile_error!("Chymeral currently supports only Linux and Windows.");

mod background;
mod config;
mod diagnostics;
mod pty;
mod scene_renderer;
mod terminal;

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use arboard::Clipboard;
use chymeral::interaction::SceneInteraction;
use chymeral::runtime::{
    EngineEvent, ExternalFile as RuntimeExternalFile, FocusEvent, InitContext, Key as EngineKey,
    KeyEvent as EngineKeyEvent, KeyState, Modifiers as EngineModifiers, NamedKey as EngineNamedKey,
    PointerButton, PointerEvent, Program, Runtime, RuntimeCommand, RuntimeResult, ScrollEvent,
    ScrollUnit, TerminalEvent, Vec2,
};
use chymeral::{
    external_drop::{ExternalFile, TerminalQuoting, format_terminal_paths},
    program_scene::{ImageHandle, ProgramSceneError, ProgramSceneResources, WindowIcon},
    protocol::{Capability, CapabilitySet, ProgramInitContext, SessionId, WindowMetadataSupport},
    resources::{Texture as ResourceTexture, TextureId, TextureStore},
    scene::{
        ImageNode, NodeContent, NodeId, NodeProperties, Point, Rect, Scene, SceneLayer, Transform,
    },
    scene_text::{TextAlign, TextLayout, TextStyle, TextWeight, expand_tabs},
    transport::{ClpServer, HostAction, HostCommandResponse, PresentationSubmission},
};
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight, Wrap, cosmic_text::Align,
};
use portable_pty::PtySize;
use wgpu::{
    CommandEncoderDescriptor, CompositeAlphaMode, DeviceDescriptor, Instance, InstanceDescriptor,
    LoadOp, MultisampleState, Operations, PresentMode, RenderPassColorAttachment,
    RenderPassDescriptor, RequestAdapterOptions, SurfaceCapabilities, SurfaceColorSpace,
    SurfaceConfiguration, TextureFormat, TextureUsages, TextureViewDescriptor,
};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    event::{ElementState, KeyEvent as WinitKeyEvent, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{Key as WinitKey, ModifiersState, NamedKey as WinitNamedKey},
    window::{Icon, Window, WindowId},
};

use crate::{
    background::BackgroundRenderer,
    config::{Appearance, FontAppearance, FontSource, Rgb, StartupSplash, TestingScene},
    diagnostics::{Diagnostics, DiagnosticsShortcuts},
    pty::Pty,
    scene_renderer::{ImageRenderer, ImageViewport},
    terminal::{Terminal, TextColor},
};

const WINDOW_WIDTH: u32 = 960;
const WINDOW_HEIGHT: u32 = 640;
const PADDING: f64 = 12.0;
const MAX_PTY_CHUNKS_PER_UPDATE: usize = 64;
const RESIZE_UPDATE_INTERVAL: Duration = Duration::from_millis(16);
const RUNTIME_TICK_INTERVAL: Duration = Duration::from_millis(16);
const BUNDLED_WINDOW_ICON: &[u8] = include_bytes!("../branding/chymeral-logo.png");

#[cfg(target_os = "linux")]
const FORCE_X11_ENV: &str = "CHYMERAL_FORCE_X11";

#[derive(Debug)]
pub enum UserEvent {
    PtyOutput,
    PtyClosed,
    ClpAction,
}

#[derive(Default)]
struct TerminalProgram {
    pending_input: Vec<Vec<u8>>,
}

impl TerminalProgram {
    fn take_pending_input(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.pending_input)
    }
}

impl Program for TerminalProgram {
    fn init(&mut self, _: &mut InitContext) -> RuntimeResult<()> {
        Ok(())
    }

    fn start(&mut self) -> RuntimeResult<()> {
        Ok(())
    }

    fn on_event(&mut self, event: &EngineEvent) -> RuntimeResult<()> {
        if let EngineEvent::Key(event) = event
            && let Some(bytes) = terminal_input(event)
        {
            self.pending_input.push(bytes);
        }
        Ok(())
    }

    fn on_command(&mut self, _: &RuntimeCommand) -> RuntimeResult<()> {
        Ok(())
    }

    fn tick(&mut self, _: Duration) -> RuntimeResult<()> {
        Ok(())
    }

    fn frame(&mut self, _: Duration) -> RuntimeResult<()> {
        Ok(())
    }
}

struct ActiveSplash {
    node: NodeId,
    texture: TextureId,
    deadline: Instant,
}

struct PendingExternalDrop {
    files: Vec<ExternalFile>,
    position: Vec2,
}

struct PendingPresentationConfirmation {
    submission: PresentationSubmission,
    committed_at: Instant,
}

struct PresentationTrace {
    enabled: bool,
    latest_input: Option<(&'static str, Instant)>,
}

impl PresentationTrace {
    fn from_environment() -> Self {
        Self {
            enabled: std::env::var_os("CHYMERAL_PRESENTATION_TRACE").is_some(),
            latest_input: None,
        }
    }

    fn record_input(&mut self, kind: &'static str, now: Instant) {
        if self.enabled {
            self.latest_input = Some((kind, now));
        }
    }

    fn record_commit(&self, submission: &PresentationSubmission, now: Instant) {
        if !self.enabled {
            return;
        }
        let received_to_commit = now.saturating_duration_since(submission.received_at());
        let latest_input = self.latest_input.map(|(kind, input_at)| {
            format!(
                ", latest_host_{kind}_to_transport_ms={:.3}",
                submission
                    .received_at()
                    .saturating_duration_since(input_at)
                    .as_secs_f64()
                    * 1_000.0
            )
        });
        eprintln!(
            "Chymeral: presentation trace id={} transport_to_commit_ms={:.3}{} client_delivery_ms=unavailable",
            submission.packet().presentation_id,
            received_to_commit.as_secs_f64() * 1_000.0,
            latest_input.unwrap_or_default(),
        );
    }

    fn record_rejected(&self, submission: &PresentationSubmission, now: Instant, code: &str) {
        if !self.enabled {
            return;
        }
        eprintln!(
            "Chymeral: presentation trace id={} transport_to_rejection_ms={:.3} code={code}",
            submission.packet().presentation_id,
            now.saturating_duration_since(submission.received_at())
                .as_secs_f64()
                * 1_000.0,
        );
    }

    fn record_presented(&self, confirmation: &PendingPresentationConfirmation, now: Instant) {
        if !self.enabled {
            return;
        }
        let transport_to_present =
            now.saturating_duration_since(confirmation.submission.received_at());
        let commit_to_present = now.saturating_duration_since(confirmation.committed_at);
        eprintln!(
            "Chymeral: presentation trace id={} commit_to_present_call_ms={:.3} transport_to_present_call_ms={:.3} scanout_timing=unavailable",
            confirmation.submission.packet().presentation_id,
            commit_to_present.as_secs_f64() * 1_000.0,
            transport_to_present.as_secs_f64() * 1_000.0,
        );
    }

    fn record_submitted(&self, confirmation: &PendingPresentationConfirmation, now: Instant) {
        if !self.enabled {
            return;
        }
        eprintln!(
            "Chymeral: presentation trace id={} commit_to_submit_ms={:.3}",
            confirmation.submission.packet().presentation_id,
            now.saturating_duration_since(confirmation.committed_at)
                .as_secs_f64()
                * 1_000.0,
        );
    }
}

struct WindowStateOptions {
    appearance: Appearance,
    startup_splash: Option<StartupSplash>,
    testing_scene: Option<TestingScene>,
    diagnostics_shortcuts: DiagnosticsShortcuts,
    available_capabilities: CapabilitySet,
    fallback_window_icon: Option<Icon>,
    window_icon_supported: bool,
}

struct WindowState {
    instance: wgpu::Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: SurfaceConfiguration,
    scale_factor: f64,
    font: FontAppearance,
    foreground: Rgb,
    font_family: String,
    text_font_families: BTreeMap<String, String>,
    background: BackgroundRenderer,
    terminal: Terminal,
    pty: Pty,
    clp_server: ClpServer,
    runtime: Runtime<TerminalProgram>,
    diagnostics: Diagnostics,
    scene: Scene,
    interaction: SceneInteraction,
    pointer_position: Vec2,
    external_file_drop_supported: bool,
    external_drop_active: bool,
    external_drop_position: Vec2,
    external_drop_target: Option<NodeId>,
    pending_external_drop: Option<PendingExternalDrop>,
    terminal_selection_active: bool,
    terminal_scroll_remainder: f64,
    textures: TextureStore,
    program_scene_resources: ProgramSceneResources,
    image_renderer: ImageRenderer,
    active_splash: Option<ActiveSplash>,
    pending_presentation_confirmations: Vec<PendingPresentationConfirmation>,
    presentation_trace: PresentationTrace,
    pending_resize: Option<PhysicalSize<u32>>,
    next_resize_update: Option<Instant>,
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    text_buffer: Buffer,
    scene_text_renderers: HashMap<NodeId, TextRenderer>,
    scene_text_buffer: Buffer,
    fallback_window_icon: Option<Icon>,
    applied_window_title: String,
    applied_window_icon: Option<ImageHandle>,
    window_icon_supported: bool,
    window: Arc<Window>,
}

impl WindowState {
    async fn new(
        window: Arc<Window>,
        event_loop: &ActiveEventLoop,
        event_proxy: EventLoopProxy<UserEvent>,
        options: WindowStateOptions,
    ) -> Result<Self> {
        let WindowStateOptions {
            appearance,
            startup_splash,
            testing_scene,
            diagnostics_shortcuts,
            available_capabilities,
            fallback_window_icon,
            window_icon_supported,
        } = options;
        let physical_size = window.inner_size();
        let scale_factor = window.scale_factor();
        let (columns, rows) = grid_size(physical_size, &appearance.font, scale_factor);
        let terminal = Terminal::new(
            columns,
            rows,
            TextColor {
                red: appearance.foreground.color.red,
                green: appearance.foreground.color.green,
                blue: appearance.foreground.color.blue,
            },
        );
        let clp_event_proxy = event_proxy.clone();
        let clp_server = ClpServer::start_with_context_and_wake(
            available_capabilities,
            ProgramInitContext::default().with_window_metadata_support(WindowMetadataSupport {
                icon_supported: window_icon_supported,
            }),
            move || {
                let _ = clp_event_proxy.send_event(UserEvent::ClpAction);
            },
        )?;
        let pty = Pty::spawn(
            pty_size(columns, rows, physical_size),
            event_proxy,
            clp_server.environment(),
        )?;
        let runtime = Runtime::new(TerminalProgram::default(), RUNTIME_TICK_INTERVAL)?;
        let diagnostics =
            Diagnostics::new(diagnostics_shortcuts, Instant::now(), runtime.metrics());
        let instance = Instance::new(InstanceDescriptor::new_with_display_handle(Box::new(
            event_loop.owned_display_handle(),
        )));
        let adapter = instance
            .request_adapter(&RequestAdapterOptions::default())
            .await
            .context("could not find a compatible GPU adapter")?;
        let (device, queue) = adapter
            .request_device(&DeviceDescriptor::default())
            .await
            .context("could not create a GPU device")?;
        let surface = instance
            .create_surface(window.clone())
            .context("could not create a rendering surface")?;
        let surface_capabilities = surface.get_capabilities(&adapter);
        let surface_format = select_surface_format(&surface_capabilities)?;
        let alpha_mode = if appearance.background.requires_window_transparency() {
            surface_capabilities
                .alpha_modes
                .iter()
                .copied()
                .find(|mode| *mode == CompositeAlphaMode::PreMultiplied)
                .unwrap_or(CompositeAlphaMode::Opaque)
        } else {
            CompositeAlphaMode::Opaque
        };
        if appearance.background.requires_window_transparency()
            && alpha_mode == CompositeAlphaMode::Opaque
        {
            eprintln!(
                "Chymeral: this compositor does not expose premultiplied window transparency; using an opaque surface"
            );
        }
        let surface_config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: physical_size.width.max(1),
            height: physical_size.height.max(1),
            present_mode: select_present_mode(&surface_capabilities)?,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
            color_space: SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &surface_config);

        let (mut font_system, font_family, text_font_families) =
            configure_font_system(&appearance.font, &appearance.scene_text_fonts)?;
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, surface_config.format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, MultisampleState::default(), None);
        let mut text_buffer = Buffer::new(
            &mut font_system,
            physical_font_metrics(&appearance.font, scale_factor),
        );
        text_buffer.set_wrap(Wrap::None);
        let mut scene_text_buffer = Buffer::new(
            &mut font_system,
            physical_font_metrics(&appearance.font, scale_factor),
        );
        scene_text_buffer.set_wrap(Wrap::None);
        let background =
            BackgroundRenderer::new(&device, &queue, &surface_config, &appearance.background)?;
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
        if let Some(testing_scene) = testing_scene.as_ref() {
            install_testing_scene(&mut scene, &mut textures, testing_scene)?;
        }
        let active_splash = if let Some(splash) = startup_splash {
            let max_dimension = device.limits().max_texture_dimension_2d;
            let texture = background::load_static_texture(&splash.path, max_dimension)?;
            let texture_id = textures.insert(texture);
            let texture = textures
                .texture(texture_id)
                .context("could not retrieve the newly registered splash texture")?;
            let node = scene.create_root(splash_properties(
                texture,
                surface_config.width,
                surface_config.height,
                scale_factor,
            )?);
            scene.set_content(
                node,
                NodeContent::Image(ImageNode::new(texture_id, splash.opacity)?),
            )?;
            let deadline = Instant::now()
                .checked_add(splash.duration)
                .context("appearance.startup_splash.duration_ms exceeds the supported duration")?;
            Some(ActiveSplash {
                node,
                texture: texture_id,
                deadline,
            })
        } else {
            None
        };
        let image_renderer = ImageRenderer::new(&device, &surface_config);
        let max_program_texture_dimension = device.limits().max_texture_dimension_2d;
        let text_layout = TextLayout::new(
            appearance.font.size,
            appearance.font.resolved_cell_width(),
            appearance.font.line_height,
            text_font_families.keys().cloned(),
        )?;

        let mut state = Self {
            instance,
            device,
            queue,
            surface,
            surface_config,
            scale_factor,
            font: appearance.font,
            foreground: appearance.foreground.color,
            font_family,
            text_font_families: text_font_families.clone(),
            background,
            terminal,
            pty,
            clp_server,
            runtime,
            diagnostics,
            scene,
            interaction: SceneInteraction::new(),
            pointer_position: Vec2::default(),
            external_file_drop_supported: available_capabilities.contains(Capability::ExternalDrop),
            external_drop_active: false,
            external_drop_position: Vec2::default(),
            external_drop_target: None,
            pending_external_drop: None,
            terminal_selection_active: false,
            terminal_scroll_remainder: 0.0,
            textures,
            program_scene_resources: ProgramSceneResources::new(max_program_texture_dimension)
                .with_text_layout(text_layout),
            image_renderer,
            active_splash,
            pending_presentation_confirmations: Vec::new(),
            presentation_trace: PresentationTrace::from_environment(),
            pending_resize: None,
            next_resize_update: None,
            font_system,
            swash_cache,
            viewport,
            atlas,
            text_renderer,
            text_buffer,
            scene_text_renderers: HashMap::new(),
            scene_text_buffer,
            fallback_window_icon,
            applied_window_title: "Chymeral".to_owned(),
            applied_window_icon: None,
            window_icon_supported,
            window,
        };
        state
            .runtime
            .initialize(InitContext::default(), Instant::now())?;
        state.sync_terminal_text(false);
        Ok(state)
    }

    fn read_pty_output(&mut self) -> bool {
        let chunks: Vec<_> = self
            .pty
            .drain_output()
            .take(MAX_PTY_CHUNKS_PER_UPDATE)
            .collect();
        if chunks.is_empty() {
            return false;
        }

        for chunk in chunks {
            self.terminal.advance(&chunk);
            for response in self.terminal.take_responses() {
                self.write_input(&response);
            }
        }
        if self.pending_resize.is_none() {
            self.sync_terminal_text(false);
        }
        true
    }

    fn sync_terminal_text(&mut self, force: bool) {
        if self.diagnostics.is_active() || (!force && !self.terminal.take_dirty()) {
            return;
        }
        if force {
            self.terminal.take_dirty();
        }

        let spans = self.terminal.render_spans();
        let padding = physical_padding(self.scale_factor);
        let width = self.surface_config.width.saturating_sub(padding * 2) as f32;
        let height = self.surface_config.height.saturating_sub(padding * 2) as f32;
        self.text_buffer.set_size(Some(width), Some(height));
        let default_attributes = Attrs::new().family(Family::Name(&self.font_family));
        self.text_buffer.set_rich_text(
            spans.iter().map(|span| {
                (
                    span.text.as_str(),
                    default_attributes
                        .clone()
                        .color(Color::rgb(
                            span.color.red,
                            span.color.green,
                            span.color.blue,
                        ))
                        .weight(if span.bold {
                            Weight::BOLD
                        } else {
                            Weight::NORMAL
                        }),
                )
            }),
            &default_attributes,
            Shaping::Advanced,
            None,
        );
        self.text_buffer
            .shape_until_scroll(&mut self.font_system, false);

        self.sync_window_metadata();
    }

    fn sync_window_metadata(&mut self) {
        let title = if self.diagnostics.is_active() {
            "Chymeral — Diagnostics".to_owned()
        } else if let Some(title) = self
            .program_scene_resources
            .active_window_metadata()
            .and_then(|metadata| metadata.title())
        {
            title.to_owned()
        } else {
            self.terminal_window_title()
        };
        if self.applied_window_title != title {
            self.window.set_title(&title);
            self.applied_window_title = title;
        }
        self.sync_window_icon();
    }

    fn terminal_window_title(&self) -> String {
        if self.terminal.title().is_empty() {
            "Chymeral".to_owned()
        } else {
            format!("Chymeral — {}", self.terminal.title())
        }
    }

    fn sync_window_icon(&mut self) {
        if !self.window_icon_supported {
            return;
        }
        let active_icon = self
            .program_scene_resources
            .active_window_metadata()
            .and_then(|metadata| metadata.icon());
        if self.applied_window_icon == active_icon {
            return;
        }
        let icon = active_icon
            .and_then(|_| {
                self.program_scene_resources
                    .active_window_icon(&self.textures)
            })
            .and_then(window_icon_from_metadata)
            .or_else(|| self.fallback_window_icon.clone());
        self.window.set_window_icon(icon.clone());
        #[cfg(target_os = "windows")]
        {
            use winit::platform::windows::WindowExtWindows;

            self.window.set_taskbar_icon(icon);
        }
        self.applied_window_icon = active_icon;
    }

    fn terminal_selection_rectangles(&self) -> Vec<Rect> {
        let cell_width = f64::from(self.font.resolved_cell_width());
        let line_height = f64::from(self.font.line_height);
        if !cell_width.is_finite()
            || !line_height.is_finite()
            || cell_width <= 0.0
            || line_height <= 0.0
        {
            return Vec::new();
        }
        self.terminal
            .selection_ranges()
            .into_iter()
            .filter_map(|range| {
                let width = f64::from(range.end_column - range.start_column) * cell_width;
                Rect::new(
                    PADDING + f64::from(range.start_column) * cell_width,
                    PADDING + f64::from(range.row) * line_height,
                    width,
                    line_height,
                )
                .ok()
            })
            .collect()
    }

    fn sync_diagnostics_text(&mut self) {
        let (_, rows) = grid_size(
            PhysicalSize::new(self.surface_config.width, self.surface_config.height),
            &self.font,
            self.scale_factor,
        );
        let Some(text) = self
            .diagnostics
            .render_text(self.runtime.metrics(), usize::from(rows))
        else {
            return;
        };

        let padding = physical_padding(self.scale_factor);
        let width = self.surface_config.width.saturating_sub(padding * 2) as f32;
        let height = self.surface_config.height.saturating_sub(padding * 2) as f32;
        self.text_buffer.set_size(Some(width), Some(height));
        let attributes = Attrs::new()
            .family(Family::Name(&self.font_family))
            .color(Color::rgb(
                self.foreground.red,
                self.foreground.green,
                self.foreground.blue,
            ));
        self.text_buffer.set_rich_text(
            std::iter::once((text.as_str(), attributes.clone())),
            &attributes,
            Shaping::Advanced,
            None,
        );
        self.text_buffer
            .shape_until_scroll(&mut self.font_system, false);
        self.sync_window_metadata();
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }

        self.pending_resize = Some(size);
        self.apply_pending_resize(Instant::now());
    }

    fn apply_pending_resize(&mut self, now: Instant) {
        if self
            .next_resize_update
            .is_some_and(|deadline| now < deadline)
        {
            return;
        }
        let Some(size) = self.pending_resize.take() else {
            return;
        };
        self.next_resize_update = Some(now + RESIZE_UPDATE_INTERVAL);

        self.surface_config.width = size.width;
        self.surface_config.height = size.height;
        self.surface.configure(&self.device, &self.surface_config);

        let (columns, rows) = grid_size(size, &self.font, self.scale_factor);
        self.terminal.resize(columns, rows);
        if let Err(error) = self.pty.resize(pty_size(columns, rows, size)) {
            eprintln!("Chymeral: {error:#}");
        }
        self.background.resize(&self.queue, &self.surface_config);
        self.center_splash();
        self.sync_terminal_text(false);
        self.clp_server.broadcast_resize(
            self.surface_config.width,
            self.surface_config.height,
            self.scale_factor,
        );
        self.window.request_redraw();
    }

    fn next_resize_deadline(&self) -> Option<Instant> {
        self.pending_resize.as_ref().and(self.next_resize_update)
    }

    fn next_splash_deadline(&self) -> Option<Instant> {
        self.active_splash.as_ref().map(|splash| splash.deadline)
    }

    fn advance_splash(&mut self, now: Instant) -> bool {
        let Some(splash) = self.active_splash.as_ref() else {
            return false;
        };
        if now < splash.deadline {
            return false;
        }
        let Some(splash) = self.active_splash.take() else {
            return false;
        };
        let _ = self.scene.remove_subtree(splash.node);
        let _ = self.textures.remove(splash.texture);
        self.reconcile_scene_interaction();
        true
    }

    fn center_splash(&mut self) {
        let Some(splash) = self.active_splash.as_ref() else {
            return;
        };
        let Some(texture) = self.textures.texture(splash.texture) else {
            return;
        };
        let Ok(properties) = splash_properties(
            texture,
            self.surface_config.width,
            self.surface_config.height,
            self.scale_factor,
        ) else {
            return;
        };
        if self.scene.set_properties(splash.node, properties).is_ok() {
            self.reconcile_scene_interaction();
        }
    }

    fn scale_factor_changed(&mut self, scale_factor: f64) {
        self.scale_factor = scale_factor;
        self.text_buffer
            .set_metrics(physical_font_metrics(&self.font, scale_factor));
        self.scene_text_buffer
            .set_metrics(physical_font_metrics(&self.font, scale_factor));
        self.resize(self.window.inner_size());
    }

    fn write_input(&self, bytes: &[u8]) {
        if let Err(error) = self.pty.write(bytes) {
            eprintln!("Chymeral: {error:#}");
        }
    }

    fn apply_clp_actions(&mut self) {
        let actions = self.clp_server.drain_host_actions();
        if actions.is_empty() {
            if self.clp_server.has_pending_presentation_packets() {
                self.window.request_redraw();
            }
            return;
        }
        let mut scene_changed = false;
        for action in actions {
            match action {
                HostAction::Command(request) => {
                    let (session_id, _, command, response_sender) = (*request).into_parts();
                    let mutates_scene = command.is_mutating();
                    let updates_window_metadata = matches!(
                        command,
                        chymeral::program_scene::ClpSceneCommand::SetWindowMetadata { .. }
                    );
                    let response = match self
                        .program_scene_resources
                        .execute_clp_with_window_icon_support(
                            session_id,
                            command,
                            &mut self.scene,
                            &mut self.textures,
                            self.window_icon_supported,
                        ) {
                        Ok(result) => {
                            if mutates_scene {
                                if let Some(superseded) =
                                    self.clp_server.discard_pending_presentation(session_id)
                                {
                                    self.clp_server.report_presentation_superseded(
                                        session_id,
                                        superseded.packet().presentation_id.clone(),
                                    );
                                }
                                self.supersede_unpresented_presentations(session_id);
                            }
                            for event in self.program_scene_resources.drain_overlap_events() {
                                self.enqueue_event(EngineEvent::Scene(event));
                            }
                            if updates_window_metadata {
                                self.sync_window_metadata();
                            }
                            scene_changed |= mutates_scene;
                            HostCommandResponse::Success(result)
                        }
                        Err(error) => HostCommandResponse::Error {
                            code: program_scene_error_code(&error),
                            message: error.to_string(),
                        },
                    };
                    let _ = response_sender.send(response);
                }
                HostAction::Cleanup { session_id } => {
                    let metadata_changed = self.program_scene_resources.cleanup_session(
                        session_id,
                        &mut self.scene,
                        &mut self.textures,
                    );
                    if metadata_changed {
                        self.sync_window_metadata();
                    }
                    self.pending_presentation_confirmations
                        .retain(|confirmation| confirmation.submission.session_id() != session_id);
                    scene_changed = true;
                }
                HostAction::SessionStarted { session_id } => {
                    self.clp_server.activate_session(
                        session_id,
                        self.surface_config.width,
                        self.surface_config.height,
                        self.scale_factor,
                    );
                }
            }
        }
        if scene_changed {
            self.reconcile_scene_interaction();
            let position = if self.external_drop_active {
                self.external_drop_position
            } else {
                self.pointer_position
            };
            self.update_external_drop_target(position);
        }
        if scene_changed || self.clp_server.has_pending_presentation_packets() {
            self.window.request_redraw();
        }
    }

    /// Commits the latest accepted visual packet for each session immediately
    /// before preparing a render. The status remains pending until the surface
    /// texture is actually presented below.
    fn apply_pending_presentations(&mut self, now: Instant) {
        let submissions = self.clp_server.drain_presentation_packets();
        if submissions.is_empty() {
            return;
        }

        let mut scene_changed = false;
        for submission in submissions {
            let session_id = submission.session_id();
            let presentation_id = submission.packet().presentation_id.clone();
            match self.program_scene_resources.present_clp(
                session_id,
                submission.packet(),
                &mut self.scene,
            ) {
                Ok(()) => {
                    self.supersede_unpresented_presentations(session_id);
                    for event in self.program_scene_resources.drain_overlap_events() {
                        self.enqueue_event(EngineEvent::Scene(event));
                    }
                    self.presentation_trace.record_commit(&submission, now);
                    self.pending_presentation_confirmations
                        .push(PendingPresentationConfirmation {
                            submission,
                            committed_at: now,
                        });
                    scene_changed = true;
                }
                Err(error) => {
                    let code = program_scene_error_code(&error);
                    self.presentation_trace
                        .record_rejected(&submission, now, code);
                    self.clp_server.report_presentation_rejected(
                        session_id,
                        presentation_id,
                        code,
                        error.to_string(),
                    );
                }
            }
        }
        if scene_changed {
            self.reconcile_scene_interaction();
            let position = if self.external_drop_active {
                self.external_drop_position
            } else {
                self.pointer_position
            };
            self.update_external_drop_target(position);
        }
    }

    /// A surface can decline a frame after a presentation packet has been
    /// committed but before `queue.present` is reached. A later visual packet
    /// or reliable scene mutation then replaces that unobservable state rather
    /// than allowing it to be confirmed as presented on a future frame.
    fn supersede_unpresented_presentations(&mut self, session_id: SessionId) {
        let mut superseded = Vec::new();
        self.pending_presentation_confirmations
            .retain(|confirmation| {
                if confirmation.submission.session_id() == session_id {
                    superseded.push(confirmation.submission.packet().presentation_id.clone());
                    false
                } else {
                    true
                }
            });
        for presentation_id in superseded {
            self.clp_server
                .report_presentation_superseded(session_id, presentation_id);
        }
    }

    fn advance_scene_animations(&mut self, now: Instant) -> bool {
        let viewport = logical_scene_viewport(
            self.surface_config.width,
            self.surface_config.height,
            self.scale_factor,
        );
        let mut visible_images = HashSet::new();
        for node in self.scene.paint_order() {
            let Some(NodeContent::AnimatedImage(image)) =
                self.scene.node(node).map(|node| node.content())
            else {
                continue;
            };
            if self
                .scene
                .visible_bounds(node)
                .ok()
                .flatten()
                .zip(viewport)
                .is_some_and(|(bounds, viewport)| bounds.intersection(viewport).is_some())
            {
                visible_images.insert(image.image());
            }
        }
        let advanced = self
            .program_scene_resources
            .animated_images_mut()
            .update_playback(now, &visible_images);
        if !advanced {
            return false;
        }

        let changed_nodes = match self
            .program_scene_resources
            .sync_animated_alpha_contacts(&mut self.scene)
        {
            Ok(changed_nodes) => changed_nodes,
            Err(error) => {
                eprintln!("Chymeral: could not update animated alpha contact: {error}");
                return true;
            }
        };
        if changed_nodes.is_empty() {
            return true;
        }
        if let Err(error) = self
            .program_scene_resources
            .reconcile_animated_alpha_contacts(&changed_nodes, &self.scene)
        {
            eprintln!("Chymeral: could not reconcile animated alpha contact: {error}");
            return true;
        }
        for event in self.program_scene_resources.drain_overlap_events() {
            self.enqueue_event(EngineEvent::Scene(event));
        }
        self.reconcile_scene_interaction();
        let position = if self.external_drop_active {
            self.external_drop_position
        } else {
            self.pointer_position
        };
        self.update_external_drop_target(position);
        true
    }

    fn next_scene_animation_frame(&self) -> Option<Instant> {
        self.program_scene_resources
            .animated_images()
            .next_frame_deadline()
    }

    fn enqueue_event(&mut self, event: EngineEvent) {
        let now = Instant::now();
        self.diagnostics.record_event(&event, now);
        match &event {
            EngineEvent::Pointer(_) => self.presentation_trace.record_input("pointer", now),
            EngineEvent::Scroll(_) => self.presentation_trace.record_input("scroll", now),
            EngineEvent::Key(_) => self.presentation_trace.record_input("key", now),
            EngineEvent::Focus(_) => self.presentation_trace.record_input("focus", now),
            _ => {}
        }
        match &event {
            EngineEvent::Scene(scene_event) => {
                self.clp_server
                    .broadcast_scene_event(scene_event, |session_id, node| {
                        self.program_scene_resources
                            .node_handle_for_session(session_id, node)
                    });
            }
            EngineEvent::Pointer(_) | EngineEvent::Scroll(_) => {
                if let Some(session_id) = self.focused_program_session() {
                    self.clp_server.send_raw_input_event(session_id, &event);
                }
            }
            _ => self.clp_server.broadcast_event(&event),
        }
        if self.runtime.enqueue_event(event) {
            self.window.request_redraw();
        }
    }

    fn handle_pointer_event(&mut self, event: PointerEvent) {
        if self.handle_terminal_selection_pointer(&event) {
            return;
        }
        self.enqueue_event(EngineEvent::Pointer(event.clone()));
        self.route_scene_pointer_event(&event);
    }

    fn handle_external_file_hover(&mut self, position: Vec2) {
        if !self.external_file_drop_supported {
            return;
        }
        self.external_drop_active = true;
        self.update_external_drop_target(position);
    }

    fn queue_external_file_drop(&mut self, path: std::path::PathBuf, position: Vec2) {
        if !self.external_file_drop_supported {
            return;
        }
        let file = match ExternalFile::from_path(path) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("Chymeral: refusing external file drop: {error}");
                return;
            }
        };
        if !self.external_drop_active {
            self.external_drop_active = true;
        }
        self.update_external_drop_target(position);
        let pending = self
            .pending_external_drop
            .get_or_insert_with(|| PendingExternalDrop {
                files: Vec::new(),
                position,
            });
        if pending.files.len() >= chymeral::external_drop::MAX_EXTERNAL_DROP_FILES {
            eprintln!(
                "Chymeral: refusing external file drop batch larger than {} files",
                chymeral::external_drop::MAX_EXTERNAL_DROP_FILES
            );
            return;
        }
        pending.files.push(file);
        pending.position = position;
    }

    fn flush_external_file_drop(&mut self) {
        let Some(drop) = self.pending_external_drop.take() else {
            return;
        };
        let target = self.external_drop_target_at(drop.position);
        if let Some(target) = target {
            self.enqueue_event(EngineEvent::Scene(
                chymeral::runtime::SceneEvent::ExternalDrop {
                    target,
                    position: drop.position,
                    files: drop
                        .files
                        .iter()
                        .map(|file| RuntimeExternalFile {
                            uri: file.uri().to_owned(),
                        })
                        .collect(),
                },
            ));
        } else {
            match format_terminal_paths(&drop.files, TerminalQuoting::platform_default()) {
                Ok(input) => {
                    if self.terminal.scroll_to_bottom() {
                        self.sync_terminal_text(false);
                    }
                    self.write_input(input.as_bytes());
                    self.window.request_redraw();
                }
                Err(error) => {
                    eprintln!("Chymeral: refusing terminal file-drop insertion: {error}");
                }
            }
        }
        self.end_external_drop_hover(drop.position);
    }

    fn cancel_external_file_hover(&mut self) {
        if !self.external_file_drop_supported {
            return;
        }
        if self.pending_external_drop.is_some() {
            self.flush_external_file_drop();
        } else {
            self.end_external_drop_hover(self.external_drop_position);
        }
    }

    fn update_external_drop_target(&mut self, position: Vec2) {
        if !self.external_file_drop_supported || !self.external_drop_active {
            return;
        }
        self.external_drop_position = position;
        let next = self.external_drop_target_at(position);
        if next == self.external_drop_target {
            if let Some(target) = next {
                self.enqueue_event(EngineEvent::Scene(
                    chymeral::runtime::SceneEvent::ExternalDropOver { target, position },
                ));
            }
            return;
        }
        if let Some(target) = self.external_drop_target {
            self.enqueue_event(EngineEvent::Scene(
                chymeral::runtime::SceneEvent::ExternalDropLeave { target, position },
            ));
        }
        self.external_drop_target = next;
        if let Some(target) = next {
            self.enqueue_event(EngineEvent::Scene(
                chymeral::runtime::SceneEvent::ExternalDropEnter { target, position },
            ));
        }
    }

    fn end_external_drop_hover(&mut self, position: Vec2) {
        self.external_drop_active = false;
        if let Some(target) = self.external_drop_target.take() {
            self.enqueue_event(EngineEvent::Scene(
                chymeral::runtime::SceneEvent::ExternalDropLeave { target, position },
            ));
        }
    }

    fn external_drop_target_at(&self, position: Vec2) -> Option<NodeId> {
        let point = Point::new(position.x, position.y).ok()?;
        self.program_scene_resources
            .external_drop_target_at(&self.scene, point)
            .ok()
            .flatten()
    }

    fn route_scene_pointer_event(&mut self, event: &PointerEvent) {
        match self.interaction.route_pointer(&self.scene, event) {
            Ok(events) => {
                for event in events {
                    self.enqueue_event(EngineEvent::Scene(event));
                }
            }
            Err(error) => eprintln!("Chymeral: could not route pointer event: {error}"),
        }
    }

    fn handle_terminal_selection_pointer(&mut self, event: &PointerEvent) -> bool {
        match event {
            PointerEvent::Press {
                button: PointerButton::Primary,
                position,
                ..
            } => {
                let Some((row, column)) = self.terminal_cell_at(*position, false) else {
                    return false;
                };
                if self.scene_node_at(*position).is_some() {
                    return false;
                }
                self.route_scene_pointer_event(event);
                self.terminal_selection_active = self.terminal.begin_selection(row, column);
                if self.terminal_selection_active {
                    self.window.request_redraw();
                }
                self.terminal_selection_active
            }
            PointerEvent::Move { position, .. } if self.terminal_selection_active => {
                if let Some((row, column)) = self.terminal_cell_at(*position, true)
                    && self.terminal.extend_selection(row, column)
                {
                    self.window.request_redraw();
                }
                true
            }
            PointerEvent::Release {
                button: PointerButton::Primary,
                position,
                modifiers,
                ..
            } if self.terminal_selection_active => {
                if let Some((row, column)) = self.terminal_cell_at(*position, true) {
                    self.terminal.extend_selection(row, column);
                }
                self.terminal_selection_active = false;
                self.route_scene_pointer_event(&PointerEvent::Move {
                    position: *position,
                    modifiers: *modifiers,
                });
                self.reconcile_scene_interaction();
                self.window.request_redraw();
                true
            }
            _ => false,
        }
    }

    fn terminal_cell_at(&self, position: Vec2, clamp: bool) -> Option<(u16, u16)> {
        if !position.x.is_finite() || !position.y.is_finite() {
            return None;
        }
        let cell_width = f64::from(self.font.resolved_cell_width());
        let line_height = f64::from(self.font.line_height);
        if !cell_width.is_finite()
            || !line_height.is_finite()
            || cell_width <= 0.0
            || line_height <= 0.0
        {
            return None;
        }
        let (columns, rows) = grid_size(
            PhysicalSize::new(self.surface_config.width, self.surface_config.height),
            &self.font,
            self.scale_factor,
        );
        let width = f64::from(columns) * cell_width;
        let height = f64::from(rows) * line_height;
        let mut x = position.x - PADDING;
        let mut y = position.y - PADDING;
        if !clamp && (x < 0.0 || x > width || y < 0.0 || y >= height) {
            return None;
        }
        x = x.clamp(0.0, width);
        y = y.clamp(0.0, (height - f64::EPSILON).max(0.0));
        let column = (x / cell_width).floor().min(f64::from(columns)) as u16;
        let row = (y / line_height).floor().min(f64::from(rows - 1)) as u16;
        Some((row, column))
    }

    fn scene_node_at(&self, position: Vec2) -> Option<NodeId> {
        let point = Point::new(position.x, position.y).ok()?;
        self.scene.hit_test(point).ok().flatten()
    }

    fn reconcile_scene_interaction(&mut self) {
        match self.interaction.reconcile(&self.scene) {
            Ok(events) => {
                for event in events {
                    self.enqueue_event(EngineEvent::Scene(event));
                }
            }
            Err(error) => eprintln!("Chymeral: could not reconcile scene interaction: {error}"),
        }
    }

    fn handle_pointer_left(&mut self) {
        if self.terminal_selection_active {
            return;
        }
        for event in self.interaction.pointer_left() {
            self.enqueue_event(EngineEvent::Scene(event));
        }
        self.cancel_external_file_hover();
    }

    fn cancel_scene_drag(&mut self) {
        self.terminal_selection_active = false;
        for event in self.interaction.window_focus_lost() {
            self.enqueue_event(EngineEvent::Scene(event));
        }
    }

    fn focused_program_session(&self) -> Option<chymeral::protocol::SessionId> {
        self.interaction
            .focused_node()
            .and_then(|node| self.program_scene_resources.session_for_node(node))
    }

    fn handle_key_event(&mut self, event: EngineKeyEvent) {
        let was_active = self.diagnostics.is_active();
        if self
            .diagnostics
            .handle_key(&event, Instant::now(), self.runtime.metrics())
        {
            if was_active && !self.diagnostics.is_active() {
                self.sync_terminal_text(true);
            }
            self.window.request_redraw();
            return;
        }
        if let Some(session_id) = self.focused_program_session() {
            self.presentation_trace.record_input("key", Instant::now());
            self.diagnostics
                .record_event(&EngineEvent::Key(event.clone()), Instant::now());
            self.clp_server.send_keyboard_event(session_id, &event);
            self.window.request_redraw();
            return;
        }
        if is_terminal_copy_shortcut(&event) {
            self.copy_terminal_selection();
            return;
        }
        if is_terminal_paste_shortcut(&event) {
            self.paste_terminal_clipboard();
            return;
        }
        if terminal_input(&event).is_some() && self.terminal.scroll_to_bottom() {
            self.sync_terminal_text(false);
            self.window.request_redraw();
        }
        self.enqueue_event(EngineEvent::Key(event));
    }

    fn copy_terminal_selection(&mut self) {
        let Some(text) = self.terminal.selected_text() else {
            return;
        };
        let result = Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text));
        if let Err(error) = result {
            eprintln!("Chymeral: could not copy terminal selection to the clipboard: {error}");
        }
    }

    fn paste_terminal_clipboard(&mut self) {
        let text = match Clipboard::new().and_then(|mut clipboard| clipboard.get_text()) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("Chymeral: could not read text from the clipboard: {error}");
                return;
            }
        };
        let bracketed = self.terminal.bracketed_paste_enabled();
        let Some(bytes) = terminal_paste_input(&text, bracketed) else {
            if text.contains('\0') {
                eprintln!("Chymeral: refusing clipboard text that contains a NUL byte");
            } else if contains_line_break(&text) && !bracketed {
                eprintln!(
                    "Chymeral: refusing multiline paste because the terminal application did not enable bracketed paste"
                );
            }
            return;
        };
        if self.terminal.scroll_to_bottom() {
            self.sync_terminal_text(false);
        }
        self.write_input(&bytes);
        self.window.request_redraw();
    }

    fn handle_scroll_event(&mut self, event: ScrollEvent) {
        if self.scene_node_at(event.position).is_none() && self.scroll_terminal_view(&event) {
            return;
        }
        self.enqueue_event(EngineEvent::Scroll(event));
    }

    fn scroll_terminal_view(&mut self, event: &ScrollEvent) -> bool {
        let delta = match event.unit {
            ScrollUnit::Line => event.delta.y,
            ScrollUnit::Pixel => event.delta.y / f64::from(self.font.line_height),
        };
        if !delta.is_finite() {
            return false;
        }
        self.terminal_scroll_remainder += delta;
        let lines = if self.terminal_scroll_remainder.is_sign_positive() {
            self.terminal_scroll_remainder.floor()
        } else {
            self.terminal_scroll_remainder.ceil()
        };
        if lines == 0.0 {
            return false;
        }
        self.terminal_scroll_remainder -= lines;
        let lines = lines.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
        if !self.terminal.scroll_view_by(lines) {
            return false;
        }
        self.sync_terminal_text(false);
        self.window.request_redraw();
        true
    }

    fn run_runtime_frame(&mut self, now: Instant) -> Result<()> {
        let metrics = self.runtime.metrics();
        self.runtime.run_frame(now)?;
        self.publish_runtime_timing(metrics);
        self.diagnostics.record_metrics(self.runtime.metrics(), now);
        self.flush_runtime_input();
        Ok(())
    }

    fn run_runtime_update(&mut self, now: Instant) -> Result<()> {
        let metrics = self.runtime.metrics();
        self.runtime.run_update(now)?;
        self.publish_runtime_timing(metrics);
        self.diagnostics.record_metrics(self.runtime.metrics(), now);
        self.flush_runtime_input();
        Ok(())
    }

    fn publish_runtime_timing(&self, previous: chymeral::runtime::RuntimeMetrics) {
        let metrics = self.runtime.metrics();
        for _ in previous.tick_count..metrics.tick_count {
            self.clp_server.broadcast_tick(RUNTIME_TICK_INTERVAL);
        }
        if metrics.frame_count > previous.frame_count {
            self.clp_server
                .broadcast_frame(metrics.last_frame_dt.unwrap_or_default());
        }
    }

    fn flush_runtime_input(&mut self) {
        let pending_input = self.runtime.program_mut().take_pending_input();
        if self.diagnostics.is_active() {
            return;
        }
        for bytes in pending_input {
            self.write_input(&bytes);
        }
    }

    fn prepare_scene_text(&mut self, node: NodeId) -> Result<bool> {
        let Some((text, bounds, ancestor_clip)) = (|| {
            let node_ref = self.scene.node(node)?;
            let NodeContent::Text(text) = node_ref.content() else {
                return None;
            };
            let bounds = self.scene.world_bounds(node).ok()?;
            self.scene.visible_bounds(node).ok()??;
            let ancestor_clip = self.scene.ancestor_clip_bounds(node).ok()?;
            Some((text.clone(), bounds, ancestor_clip))
        })() else {
            return Ok(false);
        };
        if text.metrics().width == 0.0 || text.metrics().height == 0.0 {
            return Ok(false);
        }
        let clip = match ancestor_clip {
            Some(bounds) => match text_bounds(bounds, self.scale_factor) {
                Some(clip) => clip,
                None => return Ok(false),
            },
            None => TextBounds::default(),
        };
        let style = text.style();
        let metrics = physical_text_metrics(&self.font, style, self.scale_factor);
        self.scene_text_buffer.set_metrics(metrics);
        self.scene_text_buffer.set_size(
            Some((text.metrics().width * self.scale_factor) as f32),
            Some((text.metrics().height * self.scale_factor) as f32),
        );
        let family = style
            .font_alias
            .as_ref()
            .and_then(|alias| self.text_font_families.get(alias))
            .unwrap_or(&self.font_family);
        let color = style.color.unwrap_or([
            self.foreground.red,
            self.foreground.green,
            self.foreground.blue,
            255,
        ]);
        let attributes = Attrs::new()
            .family(Family::Name(family))
            .weight(match style.weight {
                TextWeight::Normal => Weight::NORMAL,
                TextWeight::Bold => Weight::BOLD,
            });
        let text = expand_tabs(text.text());
        self.scene_text_buffer.set_text(
            &text,
            &attributes,
            Shaping::Advanced,
            Some(match style.align {
                TextAlign::Start => Align::Left,
                TextAlign::Center => Align::Center,
                TextAlign::End => Align::Right,
            }),
        );
        self.scene_text_buffer
            .shape_until_scroll(&mut self.font_system, false);
        let scene_text_renderer = self.scene_text_renderers.entry(node).or_insert_with(|| {
            TextRenderer::new(
                &mut self.atlas,
                &self.device,
                MultisampleState::default(),
                None,
            )
        });
        scene_text_renderer.prepare(
            &self.device,
            &self.queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            [TextArea {
                buffer: &self.scene_text_buffer,
                left: (bounds.x() * self.scale_factor) as f32,
                top: (bounds.y() * self.scale_factor) as f32,
                scale: 1.0,
                bounds: clip,
                default_color: Color::rgba(color[0], color[1], color[2], color[3]),
                custom_glyphs: &[],
            }],
            &mut self.swash_cache,
        )?;
        Ok(true)
    }

    fn render_scene_layer(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        layer: SceneLayer,
    ) -> Result<()> {
        let scene = &self.scene;
        self.scene_text_renderers.retain(|node, _| {
            scene
                .node(*node)
                .is_some_and(|node| matches!(node.content(), NodeContent::Text(_)))
        });
        for node in self.scene.paint_order_in_layer(layer) {
            let Some(content) = self.scene.node(node).map(|node| node.content().clone()) else {
                continue;
            };
            match content {
                NodeContent::Empty => {}
                NodeContent::Image(_) | NodeContent::AnimatedImage(_) | NodeContent::Solid(_) => {
                    let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                        label: Some("chymeral scene image pass"),
                        color_attachments: &[Some(RenderPassColorAttachment {
                            view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: Operations {
                                load: LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    self.image_renderer.render_node(&mut pass, node);
                }
                NodeContent::Text(_) => {
                    if !self.prepare_scene_text(node)? {
                        continue;
                    }
                    let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                        label: Some("chymeral scene text pass"),
                        color_attachments: &[Some(RenderPassColorAttachment {
                            view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: Operations {
                                load: LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    let Some(scene_text_renderer) = self.scene_text_renderers.get(&node) else {
                        continue;
                    };
                    scene_text_renderer.render(&self.atlas, &self.viewport, &mut pass)?;
                }
            }
        }
        Ok(())
    }

    fn redraw(&mut self) -> Result<()> {
        if self.pending_resize.is_some() {
            return Ok(());
        }
        let redraw_started_at = Instant::now();
        self.apply_pending_presentations(redraw_started_at);
        self.run_runtime_frame(redraw_started_at)?;
        self.read_pty_output();
        if self.diagnostics.is_active() {
            self.sync_diagnostics_text();
        }
        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.surface_config.width,
                height: self.surface_config.height,
            },
        );
        self.text_renderer.prepare(
            &self.device,
            &self.queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            [TextArea {
                buffer: &self.text_buffer,
                left: physical_padding(self.scale_factor) as f32,
                top: physical_padding(self.scale_factor) as f32,
                scale: 1.0,
                bounds: TextBounds {
                    left: i32::try_from(physical_padding(self.scale_factor)).unwrap_or(i32::MAX),
                    top: i32::try_from(physical_padding(self.scale_factor)).unwrap_or(i32::MAX),
                    right: i32::try_from(
                        self.surface_config
                            .width
                            .saturating_sub(physical_padding(self.scale_factor)),
                    )
                    .unwrap_or(i32::MAX),
                    bottom: i32::try_from(
                        self.surface_config
                            .height
                            .saturating_sub(physical_padding(self.scale_factor)),
                    )
                    .unwrap_or(i32::MAX),
                },
                default_color: Color::rgb(
                    self.foreground.red,
                    self.foreground.green,
                    self.foreground.blue,
                ),
                custom_glyphs: &[],
            }],
            &mut self.swash_cache,
        )?;
        self.image_renderer.prepare(
            &self.device,
            &self.queue,
            &self.scene,
            &self.textures,
            self.program_scene_resources.animated_images(),
            ImageViewport::new(
                self.surface_config.width,
                self.surface_config.height,
                self.scale_factor,
            ),
        )?;
        let terminal_selection = if self.diagnostics.is_active() {
            Vec::new()
        } else {
            self.terminal_selection_rectangles()
        };
        self.image_renderer.prepare_terminal_selection(
            &self.device,
            &self.queue,
            &terminal_selection,
            ImageViewport::new(
                self.surface_config.width,
                self.surface_config.height,
                self.scale_factor,
            ),
        )?;

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                self.window.request_redraw();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.surface_config);
                self.window.request_redraw();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                drop(frame);
                self.surface.configure(&self.device, &self.surface_config);
                self.window.request_redraw();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self
                    .instance
                    .create_surface(self.window.clone())
                    .context("could not recreate the rendering surface")?;
                self.surface.configure(&self.device, &self.surface_config);
                self.window.request_redraw();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                anyhow::bail!("the rendering surface reported a validation error");
            }
        };

        let view = frame.texture.create_view(&TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("chymeral frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("chymeral background pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(self.background.clear_color()),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.background.render(&mut pass);
        }
        self.render_scene_layer(&mut encoder, &view, SceneLayer::Underlay)?;
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("chymeral terminal selection pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.image_renderer.render_terminal_selection(&mut pass);
        }
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("chymeral terminal text pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_scissor_rect(0, 0, self.surface_config.width, self.surface_config.height);
            self.text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)?;
        }
        self.render_scene_layer(&mut encoder, &view, SceneLayer::Overlay)?;

        self.queue.submit(Some(encoder.finish()));
        let submitted_at = Instant::now();
        for confirmation in &self.pending_presentation_confirmations {
            self.presentation_trace
                .record_submitted(confirmation, submitted_at);
        }
        self.queue.present(frame);
        let presented_at = Instant::now();
        for confirmation in std::mem::take(&mut self.pending_presentation_confirmations) {
            self.presentation_trace
                .record_presented(&confirmation, presented_at);
            self.clp_server.report_presentation_presented(
                confirmation.submission.session_id(),
                confirmation.submission.packet().presentation_id.clone(),
            );
        }
        self.atlas.trim();
        Ok(())
    }
}

struct ChymeralApp {
    state: Option<WindowState>,
    modifiers: ModifiersState,
    event_proxy: EventLoopProxy<UserEvent>,
    appearance: Appearance,
    startup_splash: Option<StartupSplash>,
    testing_scene: Option<TestingScene>,
    diagnostics_shortcuts: DiagnosticsShortcuts,
    available_capabilities: CapabilitySet,
    window_icon_supported: bool,
}

impl ChymeralApp {
    fn new(
        event_proxy: EventLoopProxy<UserEvent>,
        appearance: Appearance,
        startup_splash: Option<StartupSplash>,
        testing_scene: Option<TestingScene>,
        diagnostics_shortcuts: DiagnosticsShortcuts,
        available_capabilities: CapabilitySet,
        window_icon_supported: bool,
    ) -> Self {
        Self {
            state: None,
            modifiers: ModifiersState::default(),
            event_proxy,
            appearance,
            startup_splash,
            testing_scene,
            diagnostics_shortcuts,
            available_capabilities,
            window_icon_supported,
        }
    }

    fn with_state(&mut self, action: impl FnOnce(&mut WindowState)) {
        if let Some(state) = self.state.as_mut() {
            action(state);
        }
    }
}

impl ApplicationHandler<UserEvent> for ChymeralApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let window_icon = bundled_window_icon();
        let fallback_window_icon = window_icon.clone();
        let window_attributes = Window::default_attributes()
            .with_title("Chymeral")
            .with_inner_size(LogicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT))
            .with_transparent(self.appearance.background.requires_window_transparency())
            .with_window_icon(window_icon.clone());
        #[cfg(target_os = "windows")]
        let window_attributes = {
            use winit::platform::windows::WindowAttributesExtWindows;

            window_attributes.with_taskbar_icon(window_icon)
        };
        let window = match event_loop.create_window(window_attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("Chymeral: could not create the window: {error}");
                event_loop.exit();
                return;
            }
        };
        let event_proxy = self.event_proxy.clone();

        match pollster::block_on(WindowState::new(
            window,
            event_loop,
            event_proxy,
            WindowStateOptions {
                appearance: self.appearance.clone(),
                startup_splash: self.startup_splash.clone(),
                testing_scene: self.testing_scene.clone(),
                diagnostics_shortcuts: self.diagnostics_shortcuts.clone(),
                available_capabilities: self.available_capabilities,
                fallback_window_icon,
                window_icon_supported: self.window_icon_supported,
            },
        )) {
            Ok(state) => {
                state.window.request_redraw();
                self.state = Some(state);
            }
            Err(error) => {
                eprintln!("Chymeral: {error:#}");
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyOutput => self.with_state(|state| {
                if state.read_pty_output() {
                    state.enqueue_event(EngineEvent::Terminal(TerminalEvent::OutputAvailable));
                    state.window.request_redraw();
                }
            }),
            UserEvent::PtyClosed => {
                self.with_state(|state| {
                    state.enqueue_event(EngineEvent::Terminal(TerminalEvent::Exited));
                });
                event_loop.exit();
            }
            UserEvent::ClpAction => self.with_state(WindowState::apply_clp_actions),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::Resized(size) => self.with_state(|state| state.resize(size)),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.with_state(|state| state.scale_factor_changed(scale_factor))
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let event = engine_key_event(&event, self.modifiers);
                self.with_state(|state| state.handle_key_event(event));
            }
            WindowEvent::CursorMoved { position, .. } => {
                let modifiers = engine_modifiers(self.modifiers);
                self.with_state(|state| {
                    let Some(position) = logical_pointer_position(position, state.scale_factor)
                    else {
                        return;
                    };
                    state.pointer_position = position;
                    state.handle_pointer_event(PointerEvent::Move {
                        position,
                        modifiers,
                    });
                    state.update_external_drop_target(position);
                });
            }
            WindowEvent::HoveredFile(_) => {
                self.with_state(|state| {
                    let position = external_file_drop_position(
                        &state.window,
                        state.scale_factor,
                        state.pointer_position,
                    );
                    state.handle_external_file_hover(position);
                });
            }
            WindowEvent::HoveredFileCancelled => {
                self.with_state(WindowState::cancel_external_file_hover);
            }
            WindowEvent::DroppedFile(path) => {
                self.with_state(|state| {
                    let position = external_file_drop_position(
                        &state.window,
                        state.scale_factor,
                        state.pointer_position,
                    );
                    state.queue_external_file_drop(path, position);
                });
            }
            WindowEvent::CursorLeft { .. } => {
                self.with_state(WindowState::handle_pointer_left);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let modifiers = engine_modifiers(self.modifiers);
                self.with_state(|window| {
                    let event = match state {
                        ElementState::Pressed => PointerEvent::Press {
                            button: engine_pointer_button(button),
                            position: window.pointer_position,
                            modifiers,
                        },
                        ElementState::Released => PointerEvent::Release {
                            button: engine_pointer_button(button),
                            position: window.pointer_position,
                            modifiers,
                        },
                    };
                    window.handle_pointer_event(event);
                });
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let modifiers = engine_modifiers(self.modifiers);
                self.with_state(|state| {
                    let Some((delta, unit)) = engine_scroll_delta(delta, state.scale_factor) else {
                        return;
                    };
                    state.handle_scroll_event(ScrollEvent {
                        position: state.pointer_position,
                        delta,
                        unit,
                        modifiers,
                    });
                });
            }
            WindowEvent::Focused(focused) => {
                let event = EngineEvent::Focus(engine_focus(focused));
                self.with_state(|state| {
                    state.enqueue_event(event);
                    if !focused {
                        state.cancel_scene_drag();
                        state.cancel_external_file_hover();
                    }
                });
            }
            WindowEvent::RedrawRequested => {
                let mut runtime_error = None;
                self.with_state(|state| {
                    if let Err(error) = state.redraw() {
                        if state.runtime.is_faulted() {
                            runtime_error = Some(error);
                        } else {
                            eprintln!("Chymeral: {error:#}");
                        }
                    }
                });
                if let Some(error) = runtime_error {
                    eprintln!("Chymeral: {error:#}");
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let mut runtime_error = None;
        self.with_state(|state| {
            let now = Instant::now();
            state.apply_clp_actions();
            state.flush_external_file_drop();
            state.apply_pending_resize(now);
            if state
                .runtime
                .next_tick_deadline()
                .is_some_and(|deadline| deadline <= now)
                && let Err(error) = state.run_runtime_update(now)
            {
                runtime_error = Some(error);
                return;
            }
            if state.background.advance_animation(&state.queue, now)
                && state.pending_resize.is_none()
            {
                state.window.request_redraw();
            }
            if state.advance_scene_animations(now) && state.pending_resize.is_none() {
                state.window.request_redraw();
            }
            if state.advance_splash(now) && state.pending_resize.is_none() {
                state.window.request_redraw();
            }
            if state.diagnostics.needs_continuous_redraw() && state.pending_resize.is_none() {
                state.window.request_redraw();
            }
            let deadline = [
                state.background.next_animation_frame(),
                state.next_scene_animation_frame(),
                state.next_resize_deadline(),
                state.runtime.next_tick_deadline(),
                state.next_splash_deadline(),
            ]
            .into_iter()
            .flatten()
            .min();
            if let Some(deadline) = deadline {
                event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
            } else {
                event_loop.set_control_flow(ControlFlow::Wait);
            }
        });
        if let Some(error) = runtime_error {
            eprintln!("Chymeral: {error:#}");
            event_loop.exit();
        }
    }
}

fn grid_size(size: PhysicalSize<u32>, font: &FontAppearance, scale_factor: f64) -> (u16, u16) {
    let padding = physical_padding(scale_factor);
    let width = size.width.saturating_sub(padding * 2);
    let height = size.height.saturating_sub(padding * 2);
    let scale_factor = scale_factor as f32;
    let columns =
        ((width as f32 / (font.resolved_cell_width() * scale_factor)).floor() as u32).max(1);
    let rows = ((height as f32 / (font.line_height * scale_factor)).floor() as u32).max(1);

    (
        u16::try_from(columns).unwrap_or(u16::MAX),
        u16::try_from(rows).unwrap_or(u16::MAX),
    )
}

fn program_scene_error_code(error: &ProgramSceneError) -> &'static str {
    match error {
        ProgramSceneError::InvalidNodeHandle(_)
        | ProgramSceneError::InvalidImageHandle(_)
        | ProgramSceneError::InvalidTextureHandle(_)
        | ProgramSceneError::InvalidAnimatedImageHandle(_) => "invalid_handle",
        ProgramSceneError::HandleNotOwned => "not_owner",
        ProgramSceneError::ImageInUse(_)
        | ProgramSceneError::TextureInUse(_)
        | ProgramSceneError::AnimatedImageInUse(_) => "image_in_use",
        ProgramSceneError::WindowIconMustBeStaticRgba8(_)
        | ProgramSceneError::WindowMetadata(_) => "invalid_argument",
        ProgramSceneError::TextureLimitExceeded { .. }
        | ProgramSceneError::SessionTextureLimitExceeded { .. }
        | ProgramSceneError::TextureDimensionLimitExceeded { .. }
        | ProgramSceneError::WindowIconDimensionLimitExceeded { .. }
        | ProgramSceneError::WindowIconByteLimitExceeded { .. }
        | ProgramSceneError::OverlapNodeLimitExceeded { .. }
        | ProgramSceneError::TextLimitExceeded { .. }
        | ProgramSceneError::SessionTextLimitExceeded { .. }
        | ProgramSceneError::SessionTextNodeLimitExceeded { .. }
        | ProgramSceneError::PresentationUpdateLimitExceeded { .. } => "resource_limit",
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
        | ProgramSceneError::DuplicatePresentationNode(_)
        | ProgramSceneError::InvalidPresentationUpdate(_) => "invalid_node_change",
        ProgramSceneError::Scene(_) => "invalid_node_change",
        ProgramSceneError::HandleExhausted => "host_capacity",
    }
}

fn physical_font_metrics(font: &FontAppearance, scale_factor: f64) -> Metrics {
    let scale_factor = scale_factor as f32;
    Metrics::new(font.size * scale_factor, font.line_height * scale_factor)
}

fn physical_text_metrics(font: &FontAppearance, style: &TextStyle, scale_factor: f64) -> Metrics {
    let font_size = style.size.unwrap_or(font.size);
    let size_scale = font_size / font.size;
    let line_height = style.line_height.unwrap_or(font.line_height * size_scale);
    let scale_factor = scale_factor as f32;
    Metrics::new(font_size * scale_factor, line_height * scale_factor)
}

fn text_bounds(bounds: Rect, scale_factor: f64) -> Option<TextBounds> {
    let left = bounds.x() * scale_factor;
    let top = bounds.y() * scale_factor;
    let right = (bounds.x() + bounds.width()) * scale_factor;
    let bottom = (bounds.y() + bounds.height()) * scale_factor;
    if !left.is_finite() || !top.is_finite() || !right.is_finite() || !bottom.is_finite() {
        return None;
    }
    let left = left.floor().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    let top = top.floor().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    let right = right.ceil().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    let bottom = bottom
        .ceil()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    (right > left && bottom > top).then_some(TextBounds {
        left,
        top,
        right,
        bottom,
    })
}

fn physical_padding(scale_factor: f64) -> u32 {
    (PADDING * scale_factor)
        .round()
        .clamp(0.0, f64::from(u32::MAX)) as u32
}

fn install_testing_scene(
    scene: &mut Scene,
    textures: &mut TextureStore,
    testing_scene: &TestingScene,
) -> Result<()> {
    let mut ids = HashMap::new();
    let mut remaining = testing_scene.nodes.iter().collect::<Vec<_>>();
    while !remaining.is_empty() {
        let mut next = Vec::new();
        let mut created = 0;
        for testing_node in remaining {
            let properties = testing_node_properties(testing_node)?;
            let node = match testing_node.parent.as_deref() {
                Some(parent) => match ids.get(parent) {
                    Some(parent) => scene.create_child(*parent, properties)?,
                    None => {
                        next.push(testing_node);
                        continue;
                    }
                },
                None => scene.create_root_in_layer(
                    properties,
                    testing_node.layer.unwrap_or(SceneLayer::Overlay),
                ),
            };
            let texture = ResourceTexture::from_rgba8(
                1,
                1,
                vec![
                    testing_node.color.red,
                    testing_node.color.green,
                    testing_node.color.blue,
                    255,
                ],
            )?;
            let texture = textures.insert(texture);
            scene.set_content(
                node,
                NodeContent::Image(ImageNode::new(texture, testing_node.opacity)?),
            )?;
            ids.insert(testing_node.id.as_str(), node);
            eprintln!(
                "Chymeral: testing scene node {:?} uses {node:?}",
                testing_node.id
            );
            created += 1;
        }
        if created == 0 {
            anyhow::bail!("testing_tools.scene nodes could not resolve their parent hierarchy");
        }
        remaining = next;
    }
    Ok(())
}

fn testing_node_properties(testing_node: &config::TestingNode) -> Result<NodeProperties> {
    Ok(NodeProperties {
        transform: Transform::new(testing_node.x, testing_node.y)?,
        bounds: Rect::new(0.0, 0.0, testing_node.width, testing_node.height)?,
        clip_children: testing_node.clip_children,
        z_index: testing_node.z_index,
        ..NodeProperties::default()
    })
}

fn splash_properties(
    texture: &ResourceTexture,
    surface_width: u32,
    surface_height: u32,
    scale_factor: f64,
) -> Result<NodeProperties> {
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        anyhow::bail!("window scale factor must be a positive finite number");
    }

    let logical_width = f64::from(surface_width) / scale_factor;
    let logical_height = f64::from(surface_height) / scale_factor;
    let width = f64::from(texture.width());
    let height = f64::from(texture.height());
    Ok(NodeProperties {
        transform: Transform::new(
            (logical_width - width) / 2.0,
            (logical_height - height) / 2.0,
        )?,
        bounds: Rect::new(0.0, 0.0, width, height)?,
        ..NodeProperties::default()
    })
}

fn select_surface_format(capabilities: &SurfaceCapabilities) -> Result<TextureFormat> {
    capabilities
        .formats
        .iter()
        .copied()
        .find(TextureFormat::is_srgb)
        .or_else(|| capabilities.formats.first().copied())
        .context("the selected GPU surface does not expose a usable texture format")
}

fn select_present_mode(capabilities: &SurfaceCapabilities) -> Result<PresentMode> {
    capabilities
        .present_modes
        .iter()
        .copied()
        .find(|mode| *mode == PresentMode::Fifo)
        .or_else(|| capabilities.present_modes.first().copied())
        .context("the selected GPU surface does not expose a presentation mode")
}

fn pty_size(columns: u16, rows: u16, size: PhysicalSize<u32>) -> PtySize {
    PtySize {
        rows,
        cols: columns,
        pixel_width: u16::try_from(size.width).unwrap_or(u16::MAX),
        pixel_height: u16::try_from(size.height).unwrap_or(u16::MAX),
    }
}

fn logical_pointer_position(position: PhysicalPosition<f64>, scale_factor: f64) -> Option<Vec2> {
    if !position.x.is_finite()
        || !position.y.is_finite()
        || !scale_factor.is_finite()
        || scale_factor <= 0.0
    {
        return None;
    }
    let position = Vec2 {
        x: position.x / scale_factor,
        y: position.y / scale_factor,
    };
    (position.x.is_finite() && position.y.is_finite()).then_some(position)
}

fn logical_scene_viewport(width: u32, height: u32, scale_factor: f64) -> Option<Rect> {
    if width == 0 || height == 0 || !scale_factor.is_finite() || scale_factor <= 0.0 {
        return None;
    }
    Rect::new(
        0.0,
        0.0,
        f64::from(width) / scale_factor,
        f64::from(height) / scale_factor,
    )
    .ok()
}

fn engine_key_event(event: &WinitKeyEvent, modifiers: ModifiersState) -> EngineKeyEvent {
    EngineKeyEvent {
        key: engine_key(&event.logical_key),
        text: event.text.as_ref().map(ToString::to_string),
        state: match event.state {
            ElementState::Pressed => KeyState::Pressed,
            ElementState::Released => KeyState::Released,
        },
        modifiers: engine_modifiers(modifiers),
        repeat: event.repeat,
    }
}

fn engine_key(key: &WinitKey) -> EngineKey {
    match key {
        WinitKey::Character(character) => EngineKey::Character(character.to_string()),
        WinitKey::Named(key) => EngineKey::Named(engine_named_key(key)),
        WinitKey::Dead(character) => EngineKey::Dead(*character),
        WinitKey::Unidentified(_) => EngineKey::Unidentified,
    }
}

fn engine_named_key(key: &WinitNamedKey) -> EngineNamedKey {
    match key {
        WinitNamedKey::Alt => EngineNamedKey::Alt,
        WinitNamedKey::Backspace => EngineNamedKey::Backspace,
        WinitNamedKey::Delete => EngineNamedKey::Delete,
        WinitNamedKey::End => EngineNamedKey::End,
        WinitNamedKey::Enter => EngineNamedKey::Enter,
        WinitNamedKey::Escape => EngineNamedKey::Escape,
        WinitNamedKey::Home => EngineNamedKey::Home,
        WinitNamedKey::Insert => EngineNamedKey::Insert,
        WinitNamedKey::PageDown => EngineNamedKey::PageDown,
        WinitNamedKey::PageUp => EngineNamedKey::PageUp,
        WinitNamedKey::Space => EngineNamedKey::Space,
        WinitNamedKey::Tab => EngineNamedKey::Tab,
        WinitNamedKey::ArrowDown => EngineNamedKey::ArrowDown,
        WinitNamedKey::ArrowLeft => EngineNamedKey::ArrowLeft,
        WinitNamedKey::ArrowRight => EngineNamedKey::ArrowRight,
        WinitNamedKey::ArrowUp => EngineNamedKey::ArrowUp,
        key => EngineNamedKey::Other(format!("{key:?}")),
    }
}

fn engine_modifiers(modifiers: ModifiersState) -> EngineModifiers {
    EngineModifiers {
        shift: modifiers.shift_key(),
        control: modifiers.control_key(),
        alt: modifiers.alt_key(),
        super_key: modifiers.super_key(),
    }
}

fn engine_pointer_button(button: MouseButton) -> PointerButton {
    match button {
        MouseButton::Left => PointerButton::Primary,
        MouseButton::Right => PointerButton::Secondary,
        MouseButton::Middle => PointerButton::Middle,
        MouseButton::Back => PointerButton::Back,
        MouseButton::Forward => PointerButton::Forward,
        MouseButton::Other(button) => PointerButton::Other(button),
    }
}

fn engine_scroll_delta(delta: MouseScrollDelta, scale_factor: f64) -> Option<(Vec2, ScrollUnit)> {
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        return None;
    }
    let (delta, unit) = match delta {
        MouseScrollDelta::LineDelta(x, y) => (
            Vec2 {
                x: f64::from(x),
                y: f64::from(y),
            },
            ScrollUnit::Line,
        ),
        MouseScrollDelta::PixelDelta(delta) => (
            Vec2 {
                x: delta.x / scale_factor,
                y: delta.y / scale_factor,
            },
            ScrollUnit::Pixel,
        ),
    };
    (delta.x.is_finite() && delta.y.is_finite()).then_some((delta, unit))
}

fn engine_focus(focused: bool) -> FocusEvent {
    if focused {
        FocusEvent::Gained
    } else {
        FocusEvent::Lost
    }
}

fn is_terminal_copy_shortcut(event: &EngineKeyEvent) -> bool {
    is_terminal_clipboard_shortcut(event, "c")
}

fn is_terminal_paste_shortcut(event: &EngineKeyEvent) -> bool {
    is_terminal_clipboard_shortcut(event, "v")
}

fn is_terminal_clipboard_shortcut(event: &EngineKeyEvent, character: &str) -> bool {
    event.state == KeyState::Pressed
        && !event.repeat
        && event.modifiers.control
        && event.modifiers.shift
        && !event.modifiers.alt
        && !event.modifiers.super_key
        && matches!(
            &event.key,
            EngineKey::Character(value) if value.eq_ignore_ascii_case(character)
        )
}

fn contains_line_break(text: &str) -> bool {
    text.contains('\n') || text.contains('\r')
}

fn terminal_paste_input(text: &str, bracketed_paste: bool) -> Option<Vec<u8>> {
    if text.is_empty() || text.contains('\0') || (contains_line_break(text) && !bracketed_paste) {
        return None;
    }
    if !bracketed_paste {
        return Some(text.as_bytes().to_vec());
    }
    let mut bytes = Vec::with_capacity(text.len() + 12);
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    Some(bytes)
}

fn terminal_input(event: &EngineKeyEvent) -> Option<Vec<u8>> {
    if event.state != KeyState::Pressed {
        return None;
    }
    terminal_input_with_text(&event.key, event.text.as_deref(), event.modifiers)
}

fn terminal_input_with_text(
    logical_key: &EngineKey,
    text: Option<&str>,
    modifiers: EngineModifiers,
) -> Option<Vec<u8>> {
    if matches!(logical_key, EngineKey::Character(_)) && accepts_text_input(text, modifiers) {
        return text.map(|text| text.as_bytes().to_vec());
    }

    let mut bytes = match logical_key {
        EngineKey::Character(character) => text_input(character, modifiers)?,
        EngineKey::Named(named) => named_key_input(named)?,
        _ => return None,
    };

    if modifiers.alt_key() && !matches!(logical_key, EngineKey::Named(EngineNamedKey::Alt)) {
        bytes.insert(0, b'\x1b');
    }
    Some(bytes)
}

fn accepts_text_input(text: Option<&str>, modifiers: EngineModifiers) -> bool {
    let Some(text) = text else {
        return false;
    };
    if text.is_empty() || modifiers.super_key() {
        return false;
    }
    if !modifiers.control_key() && !modifiers.alt_key() {
        return true;
    }

    modifiers.control_key()
        && modifiers.alt_key()
        && text.chars().all(|character| !character.is_control())
}

fn text_input(character: &str, modifiers: EngineModifiers) -> Option<Vec<u8>> {
    if character.is_empty() || modifiers.super_key() {
        return None;
    }
    if !modifiers.control_key() {
        return Some(character.as_bytes().to_vec());
    }

    let mut characters = character.chars();
    let character = characters.next()?;
    if characters.next().is_some() {
        return None;
    }
    match character {
        '@' | ' ' => Some(vec![0]),
        '[' => Some(vec![0x1b]),
        '\\' => Some(vec![0x1c]),
        ']' => Some(vec![0x1d]),
        '^' => Some(vec![0x1e]),
        '_' => Some(vec![0x1f]),
        character if character.is_ascii_alphabetic() => {
            Some(vec![character.to_ascii_uppercase() as u8 - b'@'])
        }
        _ => None,
    }
}

fn named_key_input(key: &EngineNamedKey) -> Option<Vec<u8>> {
    let bytes = match key {
        EngineNamedKey::Space => b" ".as_slice(),
        EngineNamedKey::Enter => b"\r".as_slice(),
        EngineNamedKey::Tab => b"\t".as_slice(),
        EngineNamedKey::Backspace => b"\x7f".as_slice(),
        EngineNamedKey::Escape => b"\x1b".as_slice(),
        EngineNamedKey::ArrowUp => b"\x1b[A".as_slice(),
        EngineNamedKey::ArrowDown => b"\x1b[B".as_slice(),
        EngineNamedKey::ArrowRight => b"\x1b[C".as_slice(),
        EngineNamedKey::ArrowLeft => b"\x1b[D".as_slice(),
        EngineNamedKey::Home => b"\x1b[H".as_slice(),
        EngineNamedKey::End => b"\x1b[F".as_slice(),
        EngineNamedKey::Insert => b"\x1b[2~".as_slice(),
        EngineNamedKey::Delete => b"\x1b[3~".as_slice(),
        EngineNamedKey::PageUp => b"\x1b[5~".as_slice(),
        EngineNamedKey::PageDown => b"\x1b[6~".as_slice(),
        _ => return None,
    };
    Some(bytes.to_vec())
}

fn configure_font_system(
    font: &FontAppearance,
    scene_text_fonts: &BTreeMap<String, FontSource>,
) -> Result<(FontSystem, String, BTreeMap<String, String>)> {
    let mut font_system = FontSystem::new();
    let family = match &font.source {
        FontSource::System(family) => family.clone(),
        FontSource::File { path, face_index } => {
            load_font_file_family(&mut font_system, path, *face_index)?
        }
    };
    let mut aliases = BTreeMap::new();
    for (alias, source) in scene_text_fonts {
        let FontSource::File { path, face_index } = source else {
            anyhow::bail!("scene text font aliases must reference font files");
        };
        let alias_family = load_font_file_family(&mut font_system, path, *face_index)?;
        if alias_family == family || aliases.values().any(|existing| existing == &alias_family) {
            anyhow::bail!(
                "scene text font alias {alias:?} resolves to a family already in use; aliases must use distinct font families"
            );
        }
        aliases.insert(alias.clone(), alias_family);
    }

    Ok((font_system, family, aliases))
}

fn load_font_file_family(
    font_system: &mut FontSystem,
    path: &std::path::Path,
    face_index: u32,
) -> Result<String> {
    let existing_faces = font_system.db().faces().count();
    font_system
        .db_mut()
        .load_font_file(path)
        .with_context(|| format!("could not load font file {}", path.display()))?;
    let selected_face = font_system
        .db()
        .faces()
        .skip(existing_faces)
        .find(|face| face.index == face_index)
        .cloned()
        .with_context(|| {
            format!(
                "font file {} does not contain face index {}",
                path.display(),
                face_index
            )
        })?;
    let family = selected_face
        .families
        .first()
        .map(|(family, _)| family.clone())
        .with_context(|| {
            format!(
                "font file {} does not provide a usable family for face index {}",
                path.display(),
                face_index
            )
        })?;
    let conflicting_faces = font_system
        .db()
        .faces()
        .filter(|face| {
            face.id != selected_face.id
                && face
                    .families
                    .iter()
                    .any(|(candidate, _)| candidate == &family)
        })
        .map(|face| face.id)
        .collect::<Vec<_>>();
    for face in conflicting_faces {
        font_system.db_mut().remove_face(face);
    }
    Ok(family)
}

fn main() -> Result<()> {
    let loaded_config = config::load()?;
    if let Some(path) = &loaded_config.path {
        eprintln!("Chymeral: loaded configuration from {}", path.display());
    }
    let event_loop = create_event_loop()?;
    let external_drop_supported = platform_supports_external_drop(&event_loop);
    report_platform_external_drop_support(external_drop_supported);
    let window_icon_supported = platform_supports_window_icon(&event_loop);
    report_platform_window_icon_support(window_icon_supported);
    let mut app = ChymeralApp::new(
        event_loop.create_proxy(),
        loaded_config.appearance,
        loaded_config.startup_splash,
        loaded_config.testing_scene,
        loaded_config.diagnostics,
        available_clp_capabilities(external_drop_supported),
        window_icon_supported,
    );
    event_loop
        .run_app(&mut app)
        .context("event loop terminated unexpectedly")
}

fn create_event_loop() -> Result<EventLoop<UserEvent>> {
    #[cfg(target_os = "linux")]
    {
        use winit::platform::x11::EventLoopBuilderExtX11;

        let mut event_loop_builder = EventLoop::<UserEvent>::with_user_event();
        if force_x11_backend() {
            eprintln!("Chymeral: forcing the X11 backend through {FORCE_X11_ENV}");
            event_loop_builder.with_x11();
        }
        event_loop_builder
            .build()
            .context("could not create the event loop")
    }

    #[cfg(target_os = "windows")]
    {
        EventLoop::<UserEvent>::with_user_event()
            .build()
            .context("could not create the event loop")
    }
}

fn bundled_window_icon() -> Option<Icon> {
    let image = match image::load_from_memory(BUNDLED_WINDOW_ICON) {
        Ok(image) => image.into_rgba8(),
        Err(error) => {
            eprintln!("Chymeral: could not decode the bundled window icon: {error}");
            return None;
        }
    };
    let (width, height) = (image.width(), image.height());
    match Icon::from_rgba(image.into_raw(), width, height) {
        Ok(icon) => Some(icon),
        Err(error) => {
            eprintln!("Chymeral: could not create the bundled window icon: {error}");
            None
        }
    }
}

fn window_icon_from_metadata(icon: WindowIcon) -> Option<Icon> {
    match Icon::from_rgba(icon.pixels, icon.width, icon.height) {
        Ok(icon) => Some(icon),
        Err(error) => {
            eprintln!("Chymeral: could not create the CLP application icon: {error}");
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn force_x11_backend() -> bool {
    matches!(std::env::var(FORCE_X11_ENV).as_deref(), Ok("1"))
}

#[cfg(target_os = "linux")]
fn platform_supports_external_drop(event_loop: &EventLoop<UserEvent>) -> bool {
    use winit::platform::wayland::EventLoopExtWayland;

    !event_loop.is_wayland()
}

#[cfg(target_os = "linux")]
fn platform_supports_window_icon(event_loop: &EventLoop<UserEvent>) -> bool {
    use winit::platform::wayland::EventLoopExtWayland;

    !event_loop.is_wayland()
}

#[cfg(target_os = "windows")]
fn platform_supports_external_drop(_: &EventLoop<UserEvent>) -> bool {
    true
}

#[cfg(target_os = "windows")]
fn platform_supports_window_icon(_: &EventLoop<UserEvent>) -> bool {
    true
}

#[cfg(target_os = "linux")]
fn report_platform_external_drop_support(external_drop_supported: bool) {
    if !external_drop_supported {
        eprintln!(
            "Chymeral: incoming external file drops are unavailable on the active Wayland backend; use an X11 session for M16 file drops"
        );
    }
}

#[cfg(target_os = "linux")]
fn report_platform_window_icon_support(window_icon_supported: bool) {
    if !window_icon_supported {
        eprintln!(
            "Chymeral: CLP application icons are unavailable on the active Wayland backend; application titles remain available"
        );
    }
}

#[cfg(target_os = "windows")]
fn report_platform_external_drop_support(_: bool) {}

#[cfg(target_os = "windows")]
fn report_platform_window_icon_support(_: bool) {}

fn available_clp_capabilities(external_drop_supported: bool) -> CapabilitySet {
    if external_drop_supported {
        CapabilitySet::ALL
    } else {
        CapabilitySet::ALL.without(Capability::ExternalDrop)
    }
}

fn external_file_drop_position(window: &Window, scale_factor: f64, fallback: Vec2) -> Vec2 {
    #[cfg(target_os = "windows")]
    {
        windows_external_file_drop_position(window, scale_factor).unwrap_or(fallback)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window, scale_factor);
        fallback
    }
}

#[cfg(target_os = "windows")]
fn windows_external_file_drop_position(window: &Window, scale_factor: f64) -> Option<Vec2> {
    use windows_sys::Win32::{
        Foundation::POINT, Graphics::Gdi::ScreenToClient, UI::WindowsAndMessaging::GetCursorPos,
    };
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let RawWindowHandle::Win32(handle) = window.window_handle().ok()?.as_raw() else {
        return None;
    };
    let mut position = POINT::default();
    // SAFETY: `position` is a valid writable `POINT` for the duration of the
    // call.
    if unsafe { GetCursorPos(&mut position) } == 0 {
        return None;
    }
    // SAFETY: the Win32 handle came from this live winit window and `position`
    // remains a valid writable `POINT`.
    if unsafe { ScreenToClient(handle.hwnd.get() as _, &mut position) } == 0 {
        return None;
    }
    external_file_drop_position_from_physical(
        PhysicalPosition::new(position.x, position.y),
        scale_factor,
    )
}

#[cfg(any(target_os = "windows", test))]
fn external_file_drop_position_from_physical(
    position: PhysicalPosition<i32>,
    scale_factor: f64,
) -> Option<Vec2> {
    logical_pointer_position(
        PhysicalPosition::new(f64::from(position.x), f64::from(position.y)),
        scale_factor,
    )
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use wgpu::{PresentMode, SurfaceCapabilities, TextureFormat};
    use winit::{
        dpi::{PhysicalPosition, PhysicalSize},
        event::{MouseButton, MouseScrollDelta},
        keyboard::{Key as WinitKey, ModifiersState, NamedKey as WinitNamedKey},
    };

    use crate::{
        TerminalProgram, available_clp_capabilities, bundled_window_icon,
        config::{Appearance, Rgb, TestingNode, TestingScene},
        engine_focus, engine_key, engine_modifiers, engine_pointer_button, engine_scroll_delta,
        external_file_drop_position_from_physical, grid_size, install_testing_scene,
        is_terminal_copy_shortcut, is_terminal_paste_shortcut, logical_pointer_position,
        logical_scene_viewport, named_key_input, pty_size, select_present_mode,
        select_surface_format, splash_properties, terminal_input, terminal_input_with_text,
        terminal_paste_input, testing_node_properties,
    };
    use chymeral::{
        protocol::Capability,
        resources::{Texture, TextureStore},
        runtime::{
            EngineEvent, FocusEvent, InitContext, Key as EngineKey, KeyEvent as EngineKeyEvent,
            KeyState, Modifiers as EngineModifiers, NamedKey as EngineNamedKey, PointerButton,
            Runtime, ScrollUnit, Vec2,
        },
        scene::{Scene, SceneLayer},
    };

    #[test]
    fn loads_the_bundled_window_icon() {
        assert!(bundled_window_icon().is_some());
    }

    #[test]
    fn keeps_the_same_grid_for_the_same_logical_window_size() {
        let font = Appearance::default().font;

        assert_eq!(
            grid_size(PhysicalSize::new(960, 640), &font, 1.0),
            grid_size(PhysicalSize::new(1920, 1280), &font, 2.0),
        );
    }

    #[test]
    fn derives_a_logical_scene_viewport_and_rejects_invalid_surfaces() {
        let viewport = logical_scene_viewport(1920, 1280, 2.0).unwrap();
        assert_eq!(viewport.width(), 960.0);
        assert_eq!(viewport.height(), 640.0);
        assert!(logical_scene_viewport(0, 1280, 2.0).is_none());
        assert!(logical_scene_viewport(1920, 1280, f64::NAN).is_none());
    }

    #[test]
    fn withholds_external_drop_capability_when_the_active_backend_cannot_deliver_it() {
        assert!(available_clp_capabilities(true).contains(Capability::ExternalDrop));
        assert!(!available_clp_capabilities(false).contains(Capability::ExternalDrop));
        assert!(available_clp_capabilities(false).contains(Capability::Scene));
    }

    #[test]
    fn prefers_an_srgb_surface_format_and_fifo_presentation() {
        let capabilities = SurfaceCapabilities {
            formats: vec![TextureFormat::Rgba8Unorm, TextureFormat::Bgra8UnormSrgb],
            present_modes: vec![PresentMode::Immediate, PresentMode::Fifo],
            ..Default::default()
        };

        assert_eq!(
            select_surface_format(&capabilities).unwrap(),
            TextureFormat::Bgra8UnormSrgb,
        );
        assert_eq!(
            select_present_mode(&capabilities).unwrap(),
            PresentMode::Fifo
        );
    }

    #[test]
    fn sends_space_for_the_named_space_key() {
        assert_eq!(named_key_input(&EngineNamedKey::Space), Some(vec![b' ']));
    }

    #[test]
    fn recognizes_only_the_unrepeated_terminal_clipboard_shortcuts() {
        let copy = EngineKeyEvent {
            key: EngineKey::Character("c".into()),
            text: Some("c".into()),
            state: KeyState::Pressed,
            modifiers: EngineModifiers {
                control: true,
                shift: true,
                ..Default::default()
            },
            repeat: false,
        };
        let paste = EngineKeyEvent {
            key: EngineKey::Character("V".into()),
            text: Some("V".into()),
            ..copy.clone()
        };

        assert!(is_terminal_copy_shortcut(&copy));
        assert!(!is_terminal_paste_shortcut(&copy));
        assert!(is_terminal_paste_shortcut(&paste));
        assert!(!is_terminal_copy_shortcut(&EngineKeyEvent {
            repeat: true,
            ..copy
        }));
    }

    #[test]
    fn protects_multiline_paste_until_bracketed_paste_is_enabled() {
        assert_eq!(
            terminal_paste_input("plain text", false),
            Some(b"plain text".to_vec())
        );
        assert_eq!(terminal_paste_input("first\nsecond", false), None);
        assert_eq!(terminal_paste_input("first\0second", true), None);
        assert_eq!(
            terminal_paste_input("first\r\nsecond", true),
            Some(b"\x1b[200~first\r\nsecond\x1b[201~".to_vec())
        );
    }

    #[test]
    fn sends_terminal_sequences_for_named_keys() {
        for (key, sequence) in [
            (EngineNamedKey::Enter, b"\r".as_slice()),
            (EngineNamedKey::Tab, b"\t".as_slice()),
            (EngineNamedKey::Backspace, b"\x7f".as_slice()),
            (EngineNamedKey::ArrowUp, b"\x1b[A".as_slice()),
            (EngineNamedKey::ArrowDown, b"\x1b[B".as_slice()),
            (EngineNamedKey::ArrowRight, b"\x1b[C".as_slice()),
            (EngineNamedKey::ArrowLeft, b"\x1b[D".as_slice()),
        ] {
            assert_eq!(
                terminal_input_with_text(&EngineKey::Named(key), None, EngineModifiers::default(),),
                Some(sequence.to_vec()),
            );
        }
    }

    #[test]
    fn prefers_the_named_backspace_sequence_over_backend_control_text() {
        assert_eq!(
            terminal_input_with_text(
                &EngineKey::Named(EngineNamedKey::Backspace),
                Some("\x08"),
                EngineModifiers::default(),
            ),
            Some(vec![0x7f]),
        );
    }

    #[test]
    fn sends_text_control_and_alt_input() {
        assert_eq!(
            terminal_input_with_text(
                &EngineKey::Character("é".into()),
                Some("é"),
                EngineModifiers::default(),
            ),
            Some("é".as_bytes().to_vec()),
        );
        assert_eq!(
            terminal_input_with_text(
                &EngineKey::Character("c".into()),
                Some("\u{3}"),
                EngineModifiers {
                    control: true,
                    ..Default::default()
                },
            ),
            Some(vec![3]),
        );
        assert_eq!(
            terminal_input_with_text(
                &EngineKey::Character("x".into()),
                Some("x"),
                EngineModifiers {
                    alt: true,
                    ..Default::default()
                },
            ),
            Some(vec![b'\x1b', b'x']),
        );
    }

    #[test]
    fn preserves_composed_and_alt_gr_text() {
        assert_eq!(
            terminal_input_with_text(
                &EngineKey::Character("a".into()),
                Some("´a"),
                EngineModifiers::default(),
            ),
            Some("´a".as_bytes().to_vec()),
        );
        assert_eq!(
            terminal_input_with_text(
                &EngineKey::Character("@".into()),
                Some("@"),
                EngineModifiers {
                    control: true,
                    alt: true,
                    ..Default::default()
                },
            ),
            Some(vec![b'@']),
        );
    }

    #[test]
    fn normalizes_backend_input_without_exposing_backend_types() {
        assert_eq!(
            engine_key(&WinitKey::Character("é".into())),
            EngineKey::Character("é".into())
        );
        assert_eq!(
            engine_key(&WinitKey::Named(WinitNamedKey::Enter)),
            EngineKey::Named(EngineNamedKey::Enter)
        );
        assert_eq!(
            engine_modifiers(ModifiersState::SHIFT | ModifiersState::CONTROL),
            EngineModifiers {
                shift: true,
                control: true,
                ..Default::default()
            }
        );
        assert_eq!(
            engine_pointer_button(MouseButton::Other(7)),
            PointerButton::Other(7)
        );
        assert_eq!(
            engine_scroll_delta(MouseScrollDelta::LineDelta(1.5, -2.0), 2.0),
            Some((Vec2 { x: 1.5, y: -2.0 }, ScrollUnit::Line))
        );
        assert_eq!(engine_focus(true), FocusEvent::Gained);
        assert_eq!(engine_focus(false), FocusEvent::Lost);
    }

    #[test]
    fn normalizes_pointer_and_pixel_scroll_coordinates_to_logical_space() {
        assert_eq!(
            logical_pointer_position(PhysicalPosition::new(200.0, 100.0), 2.0),
            Some(Vec2 { x: 100.0, y: 50.0 })
        );
        assert_eq!(
            external_file_drop_position_from_physical(PhysicalPosition::new(200, 100), 2.0),
            Some(Vec2 { x: 100.0, y: 50.0 })
        );
        assert_eq!(
            engine_scroll_delta(
                MouseScrollDelta::PixelDelta(PhysicalPosition::new(80.0, -40.0)),
                2.0,
            ),
            Some((Vec2 { x: 40.0, y: -20.0 }, ScrollUnit::Pixel))
        );
        assert_eq!(
            logical_pointer_position(PhysicalPosition::new(f64::NAN, 0.0), 1.0),
            None
        );
    }

    #[test]
    fn routes_canonical_key_events_through_the_runtime_to_the_terminal() {
        let now = Instant::now();
        let mut runtime =
            Runtime::new(TerminalProgram::default(), Duration::from_millis(16)).unwrap();
        runtime.initialize(InitContext::default(), now).unwrap();
        let pressed = EngineKeyEvent {
            key: EngineKey::Character("a".into()),
            text: Some("a".into()),
            state: KeyState::Pressed,
            modifiers: EngineModifiers::default(),
            repeat: false,
        };
        let released = EngineKeyEvent {
            state: KeyState::Released,
            ..pressed.clone()
        };

        assert!(runtime.enqueue_event(EngineEvent::Key(pressed)));
        assert!(runtime.enqueue_event(EngineEvent::Key(released.clone())));
        runtime.run_update(now).unwrap();

        assert_eq!(runtime.program_mut().take_pending_input(), vec![vec![b'a']]);
        assert_eq!(terminal_input(&released), None);
    }

    #[test]
    fn derives_pty_dimensions_from_the_terminal_grid() {
        let size = pty_size(80, 24, PhysicalSize::new(960, 640));

        assert_eq!(size.cols, 80);
        assert_eq!(size.rows, 24);
        assert_eq!(size.pixel_width, 960);
        assert_eq!(size.pixel_height, 640);
    }

    #[test]
    fn centers_a_splash_at_its_natural_logical_size() {
        let texture = Texture::from_rgba8(20, 10, vec![0; 20 * 10 * 4]).unwrap();

        let properties = splash_properties(&texture, 200, 120, 2.0).unwrap();

        assert_eq!(properties.transform.x(), 40.0);
        assert_eq!(properties.transform.y(), 25.0);
        assert_eq!(
            properties.bounds,
            chymeral::scene::Rect::new(0.0, 0.0, 20.0, 10.0).unwrap()
        );
    }

    #[test]
    fn builds_testing_nodes_in_logical_scene_coordinates() {
        let properties = testing_node_properties(&TestingNode {
            id: "source".to_owned(),
            parent: None,
            x: 24.0,
            y: 48.0,
            width: 120.0,
            height: 80.0,
            color: Rgb::new(255, 102, 0),
            opacity: 0.75,
            clip_children: false,
            z_index: 3,
            layer: None,
        })
        .unwrap();

        assert_eq!(properties.transform.x(), 24.0);
        assert_eq!(properties.transform.y(), 48.0);
        assert_eq!(
            properties.bounds,
            chymeral::scene::Rect::new(0.0, 0.0, 120.0, 80.0).unwrap()
        );
        assert_eq!(properties.z_index, 3);
    }

    #[test]
    fn installs_testing_nodes_after_their_parent_even_when_declared_first() {
        let testing_scene = TestingScene {
            nodes: vec![
                TestingNode {
                    id: "child".to_owned(),
                    parent: Some("root".to_owned()),
                    x: 80.0,
                    y: 0.0,
                    width: 80.0,
                    height: 80.0,
                    color: Rgb::new(0, 102, 255),
                    opacity: 1.0,
                    clip_children: false,
                    z_index: 1,
                    layer: None,
                },
                TestingNode {
                    id: "root".to_owned(),
                    parent: None,
                    x: 20.0,
                    y: 40.0,
                    width: 100.0,
                    height: 100.0,
                    color: Rgb::new(255, 102, 0),
                    opacity: 0.5,
                    clip_children: true,
                    z_index: 0,
                    layer: Some(SceneLayer::Underlay),
                },
            ],
        };
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();

        install_testing_scene(&mut scene, &mut textures, &testing_scene).unwrap();

        let root = scene.roots()[0];
        let child = scene.node(root).unwrap().children()[0];
        assert_eq!(scene.node(root).unwrap().layer(), SceneLayer::Underlay);
        assert!(scene.node(root).unwrap().properties().clip_children);
        assert_eq!(scene.node(child).unwrap().layer(), SceneLayer::Underlay);
        assert_eq!(textures.len(), 2);
    }
}

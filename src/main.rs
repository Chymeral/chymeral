#[cfg(not(any(target_os = "linux", target_os = "windows")))]
compile_error!("Moditty currently supports only Linux and Windows.");

mod background;
mod config;
mod diagnostics;
mod pty;
mod scene_renderer;
mod terminal;

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight, Wrap,
};
use moditty::interaction::SceneInteraction;
use moditty::runtime::{
    EngineEvent, FocusEvent, InitContext, Key as EngineKey, KeyEvent as EngineKeyEvent, KeyState,
    Modifiers as EngineModifiers, NamedKey as EngineNamedKey, PointerButton, PointerEvent, Program,
    Runtime, RuntimeCommand, RuntimeResult, ScrollEvent, ScrollUnit, TerminalEvent, Vec2,
};
use moditty::{
    resources::{Texture as ResourceTexture, TextureId, TextureStore},
    scene::{ImageNode, NodeContent, NodeId, NodeProperties, Rect, Scene, Transform},
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
    window::{Window, WindowId},
};

use crate::{
    background::BackgroundRenderer,
    config::{Appearance, FontAppearance, FontSource, Rgb, StartupSplash},
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

#[derive(Debug)]
pub enum UserEvent {
    PtyOutput,
    PtyClosed,
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
    background: BackgroundRenderer,
    terminal: Terminal,
    pty: Pty,
    runtime: Runtime<TerminalProgram>,
    diagnostics: Diagnostics,
    scene: Scene,
    interaction: SceneInteraction,
    pointer_position: Vec2,
    textures: TextureStore,
    image_renderer: ImageRenderer,
    active_splash: Option<ActiveSplash>,
    pending_resize: Option<PhysicalSize<u32>>,
    next_resize_update: Option<Instant>,
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    text_buffer: Buffer,
    window: Arc<Window>,
}

impl WindowState {
    async fn new(
        window: Arc<Window>,
        event_loop: &ActiveEventLoop,
        event_proxy: EventLoopProxy<UserEvent>,
        appearance: Appearance,
        startup_splash: Option<StartupSplash>,
        diagnostics_shortcuts: DiagnosticsShortcuts,
    ) -> Result<Self> {
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
        let pty = Pty::spawn(pty_size(columns, rows, physical_size), event_proxy)?;
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
                "Moditty: this compositor does not expose premultiplied window transparency; using an opaque surface"
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

        let (mut font_system, font_family) = configure_font_system(&appearance.font)?;
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
        let background =
            BackgroundRenderer::new(&device, &queue, &surface_config, &appearance.background)?;
        let mut scene = Scene::new();
        let mut textures = TextureStore::new();
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
            background,
            terminal,
            pty,
            runtime,
            diagnostics,
            scene,
            interaction: SceneInteraction::new(),
            pointer_position: Vec2::default(),
            textures,
            image_renderer,
            active_splash,
            pending_resize: None,
            next_resize_update: None,
            font_system,
            swash_cache,
            viewport,
            atlas,
            text_renderer,
            text_buffer,
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

        if self.terminal.title().is_empty() {
            self.window.set_title("Moditty");
        } else {
            self.window
                .set_title(&format!("Moditty — {}", self.terminal.title()));
        }
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
        self.window.set_title("Moditty — Diagnostics");
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
            eprintln!("Moditty: {error:#}");
        }
        self.background.resize(&self.queue, &self.surface_config);
        self.center_splash();
        self.sync_terminal_text(false);
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
        self.resize(self.window.inner_size());
    }

    fn write_input(&self, bytes: &[u8]) {
        if let Err(error) = self.pty.write(bytes) {
            eprintln!("Moditty: {error:#}");
        }
    }

    fn enqueue_event(&mut self, event: EngineEvent) {
        self.diagnostics.record_event(&event, Instant::now());
        if self.runtime.enqueue_event(event) {
            self.window.request_redraw();
        }
    }

    fn handle_pointer_event(&mut self, event: PointerEvent) {
        self.enqueue_event(EngineEvent::Pointer(event.clone()));
        match self.interaction.route_pointer(&self.scene, &event) {
            Ok(events) => {
                for event in events {
                    self.enqueue_event(EngineEvent::Scene(event));
                }
            }
            Err(error) => eprintln!("Moditty: could not route pointer event: {error}"),
        }
    }

    fn reconcile_scene_interaction(&mut self) {
        match self.interaction.reconcile(&self.scene) {
            Ok(events) => {
                for event in events {
                    self.enqueue_event(EngineEvent::Scene(event));
                }
            }
            Err(error) => eprintln!("Moditty: could not reconcile scene interaction: {error}"),
        }
    }

    fn handle_pointer_left(&mut self) {
        for event in self.interaction.pointer_left() {
            self.enqueue_event(EngineEvent::Scene(event));
        }
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
        self.enqueue_event(EngineEvent::Key(event));
    }

    fn run_runtime_frame(&mut self, now: Instant) -> Result<()> {
        self.runtime.run_frame(now)?;
        self.diagnostics.record_metrics(self.runtime.metrics(), now);
        self.flush_runtime_input();
        Ok(())
    }

    fn run_runtime_update(&mut self, now: Instant) -> Result<()> {
        self.runtime.run_update(now)?;
        self.diagnostics.record_metrics(self.runtime.metrics(), now);
        self.flush_runtime_input();
        Ok(())
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

    fn redraw(&mut self) -> Result<()> {
        if self.pending_resize.is_some() {
            return Ok(());
        }
        self.run_runtime_frame(Instant::now())?;
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
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
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
                label: Some("moditty frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("moditty text pass"),
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
            self.text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)?;
            self.image_renderer.render(&mut pass);
        }

        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
        self.atlas.trim();
        Ok(())
    }
}

struct ModittyApp {
    state: Option<WindowState>,
    modifiers: ModifiersState,
    event_proxy: EventLoopProxy<UserEvent>,
    appearance: Appearance,
    startup_splash: Option<StartupSplash>,
    diagnostics_shortcuts: DiagnosticsShortcuts,
}

impl ModittyApp {
    fn new(
        event_proxy: EventLoopProxy<UserEvent>,
        appearance: Appearance,
        startup_splash: Option<StartupSplash>,
        diagnostics_shortcuts: DiagnosticsShortcuts,
    ) -> Self {
        Self {
            state: None,
            modifiers: ModifiersState::default(),
            event_proxy,
            appearance,
            startup_splash,
            diagnostics_shortcuts,
        }
    }

    fn with_state(&mut self, action: impl FnOnce(&mut WindowState)) {
        if let Some(state) = self.state.as_mut() {
            action(state);
        }
    }
}

impl ApplicationHandler<UserEvent> for ModittyApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let window_attributes = Window::default_attributes()
            .with_title("Moditty")
            .with_inner_size(LogicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT))
            .with_transparent(self.appearance.background.requires_window_transparency());
        let window = match event_loop.create_window(window_attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("Moditty: could not create the window: {error}");
                event_loop.exit();
                return;
            }
        };
        let event_proxy = self.event_proxy.clone();

        match pollster::block_on(WindowState::new(
            window,
            event_loop,
            event_proxy,
            self.appearance.clone(),
            self.startup_splash.clone(),
            self.diagnostics_shortcuts.clone(),
        )) {
            Ok(state) => {
                state.window.request_redraw();
                self.state = Some(state);
            }
            Err(error) => {
                eprintln!("Moditty: {error:#}");
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
                    state.enqueue_event(EngineEvent::Scroll(ScrollEvent {
                        position: state.pointer_position,
                        delta,
                        unit,
                        modifiers,
                    }));
                });
            }
            WindowEvent::Focused(focused) => {
                let event = EngineEvent::Focus(engine_focus(focused));
                self.with_state(|state| state.enqueue_event(event));
            }
            WindowEvent::RedrawRequested => {
                let mut runtime_error = None;
                self.with_state(|state| {
                    if let Err(error) = state.redraw() {
                        if state.runtime.is_faulted() {
                            runtime_error = Some(error);
                        } else {
                            eprintln!("Moditty: {error:#}");
                        }
                    }
                });
                if let Some(error) = runtime_error {
                    eprintln!("Moditty: {error:#}");
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
            if state.advance_splash(now) && state.pending_resize.is_none() {
                state.window.request_redraw();
            }
            if state.diagnostics.needs_continuous_redraw() && state.pending_resize.is_none() {
                state.window.request_redraw();
            }
            let deadline = [
                state.background.next_animation_frame(),
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
            eprintln!("Moditty: {error:#}");
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

fn physical_font_metrics(font: &FontAppearance, scale_factor: f64) -> Metrics {
    let scale_factor = scale_factor as f32;
    Metrics::new(font.size * scale_factor, font.line_height * scale_factor)
}

fn physical_padding(scale_factor: f64) -> u32 {
    (PADDING * scale_factor)
        .round()
        .clamp(0.0, f64::from(u32::MAX)) as u32
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

fn configure_font_system(font: &FontAppearance) -> Result<(FontSystem, String)> {
    let mut font_system = FontSystem::new();
    let family = match &font.source {
        FontSource::System(family) => family.clone(),
        FontSource::File { path, face_index } => {
            let existing_faces = font_system.db().faces().count();
            font_system
                .db_mut()
                .load_font_file(path)
                .with_context(|| format!("could not load font file {}", path.display()))?;
            let selected_face = font_system
                .db()
                .faces()
                .skip(existing_faces)
                .find(|face| face.index == *face_index)
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
            family
        }
    };

    Ok((font_system, family))
}

fn main() -> Result<()> {
    let loaded_config = config::load()?;
    if let Some(path) = &loaded_config.path {
        eprintln!("Moditty: loaded configuration from {}", path.display());
    }
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .context("could not create the event loop")?;
    let mut app = ModittyApp::new(
        event_loop.create_proxy(),
        loaded_config.appearance,
        loaded_config.startup_splash,
        loaded_config.diagnostics,
    );
    event_loop
        .run_app(&mut app)
        .context("event loop terminated unexpectedly")
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
        TerminalProgram, config::Appearance, engine_focus, engine_key, engine_modifiers,
        engine_pointer_button, engine_scroll_delta, grid_size, logical_pointer_position,
        named_key_input, pty_size, select_present_mode, select_surface_format, splash_properties,
        terminal_input, terminal_input_with_text,
    };
    use moditty::{
        resources::Texture,
        runtime::{
            EngineEvent, FocusEvent, InitContext, Key as EngineKey, KeyEvent as EngineKeyEvent,
            KeyState, Modifiers as EngineModifiers, NamedKey as EngineNamedKey, PointerButton,
            Runtime, ScrollUnit, Vec2,
        },
    };

    #[test]
    fn keeps_the_same_grid_for_the_same_logical_window_size() {
        let font = Appearance::default().font;

        assert_eq!(
            grid_size(PhysicalSize::new(960, 640), &font, 1.0),
            grid_size(PhysicalSize::new(1920, 1280), &font, 2.0),
        );
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
            moditty::scene::Rect::new(0.0, 0.0, 20.0, 10.0).unwrap()
        );
    }
}

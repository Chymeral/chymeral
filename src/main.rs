#[cfg(not(any(target_os = "linux", target_os = "windows")))]
compile_error!("Moditty currently supports only Linux and Windows.");

mod background;
mod config;
mod pty;
mod terminal;

use std::sync::Arc;

use anyhow::{Context, Result};
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight, Wrap,
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
    dpi::{LogicalSize, PhysicalSize},
    event::{ElementState, KeyEvent, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{Window, WindowId},
};

use crate::{
    background::BackgroundRenderer,
    config::{Appearance, FontAppearance, FontSource, Rgb},
    pty::Pty,
    terminal::{Terminal, TextColor},
};

const WINDOW_WIDTH: u32 = 960;
const WINDOW_HEIGHT: u32 = 640;
const PADDING: f64 = 12.0;
const MAX_PTY_CHUNKS_PER_UPDATE: usize = 64;

#[derive(Debug)]
pub enum UserEvent {
    PtyOutput,
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
            font_system,
            swash_cache,
            viewport,
            atlas,
            text_renderer,
            text_buffer,
            window,
        };
        state.sync_terminal_text();
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
        self.sync_terminal_text();
        true
    }

    fn sync_terminal_text(&mut self) {
        if !self.terminal.take_dirty() {
            return;
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

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }

        self.surface_config.width = size.width;
        self.surface_config.height = size.height;
        self.surface.configure(&self.device, &self.surface_config);

        let (columns, rows) = grid_size(size, &self.font, self.scale_factor);
        self.terminal.resize(columns, rows);
        if let Err(error) = self.pty.resize(pty_size(columns, rows, size)) {
            eprintln!("Moditty: {error:#}");
        }
        self.background.resize(&self.queue, &self.surface_config);
        self.sync_terminal_text();
        self.window.request_redraw();
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

    fn redraw(&mut self) -> Result<()> {
        self.read_pty_output();
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
}

impl ModittyApp {
    fn new(event_proxy: EventLoopProxy<UserEvent>, appearance: Appearance) -> Self {
        Self {
            state: None,
            modifiers: ModifiersState::default(),
            event_proxy,
            appearance,
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

    fn user_event(&mut self, _: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::PtyOutput => self.with_state(|state| {
                if state.read_pty_output() {
                    state.window.request_redraw();
                }
            }),
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
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if let Some(bytes) = terminal_input(&event, self.modifiers) {
                    self.with_state(|state| state.write_input(&bytes));
                }
            }
            WindowEvent::RedrawRequested => self.with_state(|state| {
                if let Err(error) = state.redraw() {
                    eprintln!("Moditty: {error:#}");
                }
            }),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.with_state(|state| {
            if state
                .background
                .advance_animation(&state.queue, std::time::Instant::now())
            {
                state.window.request_redraw();
            }
            if let Some(deadline) = state.background.next_animation_frame() {
                event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
            } else {
                event_loop.set_control_flow(ControlFlow::Wait);
            }
        });
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

fn terminal_input(event: &KeyEvent, modifiers: ModifiersState) -> Option<Vec<u8>> {
    let mut bytes = match &event.logical_key {
        Key::Character(character) => text_input(character, modifiers)?,
        Key::Named(named) => named_key_input(*named)?,
        _ => return None,
    };

    if modifiers.alt_key() && !matches!(event.logical_key, Key::Named(NamedKey::Alt)) {
        bytes.insert(0, b'\x1b');
    }
    Some(bytes)
}

fn text_input(character: &str, modifiers: ModifiersState) -> Option<Vec<u8>> {
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

fn named_key_input(key: NamedKey) -> Option<Vec<u8>> {
    let bytes = match key {
        NamedKey::Space => b" ".as_slice(),
        NamedKey::Enter => b"\r".as_slice(),
        NamedKey::Tab => b"\t".as_slice(),
        NamedKey::Backspace => b"\x7f".as_slice(),
        NamedKey::Escape => b"\x1b".as_slice(),
        NamedKey::ArrowUp => b"\x1b[A".as_slice(),
        NamedKey::ArrowDown => b"\x1b[B".as_slice(),
        NamedKey::ArrowRight => b"\x1b[C".as_slice(),
        NamedKey::ArrowLeft => b"\x1b[D".as_slice(),
        NamedKey::Home => b"\x1b[H".as_slice(),
        NamedKey::End => b"\x1b[F".as_slice(),
        NamedKey::Insert => b"\x1b[2~".as_slice(),
        NamedKey::Delete => b"\x1b[3~".as_slice(),
        NamedKey::PageUp => b"\x1b[5~".as_slice(),
        NamedKey::PageDown => b"\x1b[6~".as_slice(),
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
    let mut app = ModittyApp::new(event_loop.create_proxy(), loaded_config.appearance);
    event_loop
        .run_app(&mut app)
        .context("event loop terminated unexpectedly")
}

#[cfg(test)]
mod tests {
    use wgpu::{PresentMode, SurfaceCapabilities, TextureFormat};
    use winit::dpi::PhysicalSize;
    use winit::keyboard::NamedKey;

    use crate::{
        config::Appearance, grid_size, named_key_input, select_present_mode, select_surface_format,
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
        assert_eq!(named_key_input(NamedKey::Space), Some(vec![b' ']));
    }
}

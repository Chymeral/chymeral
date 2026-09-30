use std::{
    borrow::Cow,
    fs::File,
    io::BufReader,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use image::{
    AnimationDecoder, DynamicImage, ImageDecoder, ImageReader, Limits, codecs::gif::GifDecoder,
};
use wgpu::{
    AddressMode, BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout,
    BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource, BindingType, BlendState,
    Buffer, BufferBindingType, BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites,
    Device, Extent3d, FilterMode, FragmentState, MipmapFilterMode, MultisampleState, Origin3d,
    PipelineCompilationOptions, PipelineLayoutDescriptor, PrimitiveState, Queue, RenderPass,
    RenderPipeline, RenderPipelineDescriptor, SamplerBindingType, SamplerDescriptor,
    ShaderModuleDescriptor, ShaderSource, ShaderStages, SurfaceConfiguration,
    TexelCopyBufferLayout, TexelCopyTextureInfo, Texture, TextureAspect, TextureDescriptor,
    TextureDimension, TextureFormat, TextureSampleType, TextureUsages, TextureViewDescriptor,
    TextureViewDimension, VertexState,
};

use crate::config::{Background, ImageFit, Rgb};
use chymeral::resources::Texture as ResourceTexture;

const MAX_IMAGE_DIMENSION: u32 = 8_192;
const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_GIF_FRAMES: usize = 300;
const MAX_GIF_BYTES: u64 = 128 * 1024 * 1024;

pub fn load_static_texture(path: &Path, max_dimension: u32) -> Result<ResourceTexture> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .context("startup splash image must use a PNG or JPEG extension")?;
    if !matches!(extension.as_str(), "png" | "jpg" | "jpeg") {
        bail!("startup splash image must use PNG or JPEG; animated images are not supported");
    }

    let mut reader = ImageReader::open(path)
        .with_context(|| format!("could not open startup splash image {}", path.display()))?;
    reader.limits(image_limits(max_dimension));
    let image = reader
        .decode()
        .with_context(|| format!("could not decode startup splash image {}", path.display()))?
        .into_rgba8();
    ResourceTexture::from_rgba8(image.width(), image.height(), image.into_raw())
        .map_err(anyhow::Error::from)
}

const IMAGE_SHADER: &str = r#"
struct ImageTransform {
    scale: vec2<f32>,
    offset: vec2<f32>,
}

@group(0) @binding(0)
var background_texture: texture_2d<f32>;

@group(0) @binding(1)
var background_sampler: sampler;

@group(0) @binding(2)
var<uniform> image_transform: ImageTransform;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var uvs = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(2.0, 1.0),
        vec2<f32>(0.0, -1.0),
    );
    var output: VertexOutput;
    output.position = vec4<f32>(positions[index], 0.0, 1.0);
    output.uv = uvs[index];
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let uv = input.uv * image_transform.scale + image_transform.offset;
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) {
        discard;
    }
    return textureSample(background_texture, background_sampler, uv);
}
"#;

pub enum BackgroundRenderer {
    Color { color: Rgb, opacity: f32 },
    Image(ImageBackground),
}

impl BackgroundRenderer {
    pub fn new(
        device: &Device,
        queue: &Queue,
        surface_config: &SurfaceConfiguration,
        background: &Background,
    ) -> Result<Self> {
        match background {
            Background::Color { color, opacity } => Ok(Self::Color {
                color: *color,
                opacity: *opacity,
            }),
            Background::Image { path, opacity, fit } => Ok(Self::Image(ImageBackground::new(
                device,
                queue,
                surface_config,
                path,
                *opacity,
                *fit,
            )?)),
        }
    }

    pub fn resize(&mut self, queue: &Queue, surface_config: &SurfaceConfiguration) {
        if let Self::Image(image) = self {
            image.update_transform(queue, surface_config.width, surface_config.height);
        }
    }

    pub fn advance_animation(&mut self, queue: &Queue, now: Instant) -> bool {
        match self {
            Self::Color { .. } => false,
            Self::Image(image) => image.advance_animation(queue, now),
        }
    }

    pub fn next_animation_frame(&self) -> Option<Instant> {
        match self {
            Self::Color { .. } => None,
            Self::Image(image) => image.next_frame,
        }
    }

    pub fn clear_color(&self) -> wgpu::Color {
        match self {
            Self::Color { color, opacity } => {
                let opacity = f64::from(*opacity);
                wgpu::Color {
                    r: f64::from(color.red) / 255.0 * opacity,
                    g: f64::from(color.green) / 255.0 * opacity,
                    b: f64::from(color.blue) / 255.0 * opacity,
                    a: opacity,
                }
            }
            Self::Image(_) => wgpu::Color::TRANSPARENT,
        }
    }

    pub fn render<'a>(&'a self, pass: &mut RenderPass<'a>) {
        if let Self::Image(image) = self {
            image.render(pass);
        }
    }
}

pub struct ImageBackground {
    texture: Texture,
    bind_group: BindGroup,
    transform_buffer: Buffer,
    pipeline: RenderPipeline,
    size: Extent3d,
    fit: ImageFit,
    frames: Vec<ImageFrame>,
    current_frame: usize,
    next_frame: Option<Instant>,
}

impl ImageBackground {
    fn new(
        device: &Device,
        queue: &Queue,
        surface_config: &SurfaceConfiguration,
        path: &Path,
        opacity: f32,
        fit: ImageFit,
    ) -> Result<Self> {
        let max_dimension = device
            .limits()
            .max_texture_dimension_2d
            .min(MAX_IMAGE_DIMENSION);
        let decoded = DecodedImage::load(path, opacity, max_dimension)?;
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("chymeral background image"),
            size: decoded.size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let texture_view = texture.create_view(&TextureViewDescriptor::default());
        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("chymeral background sampler"),
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            address_mode_w: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let transform_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("chymeral background transform"),
            size: 16,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_layout = background_bind_group_layout(device);
        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("chymeral background bind group"),
            layout: &bind_group_layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&texture_view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&sampler),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: transform_buffer.as_entire_binding(),
                },
            ],
        });
        let pipeline = background_pipeline(device, surface_config.format, &bind_group_layout);
        let next_frame =
            (decoded.frames.len() > 1).then(|| Instant::now() + decoded.frames[0].duration);

        let image = Self {
            texture,
            bind_group,
            transform_buffer,
            pipeline,
            size: decoded.size,
            fit,
            frames: decoded.frames,
            current_frame: 0,
            next_frame,
        };
        image.upload_current_frame(queue);
        image.update_transform(queue, surface_config.width, surface_config.height);
        Ok(image)
    }

    fn update_transform(&self, queue: &Queue, width: u32, height: u32) {
        queue.write_buffer(
            &self.transform_buffer,
            0,
            &transform_bytes(image_transform(
                self.size.width,
                self.size.height,
                width,
                height,
                self.fit,
            )),
        );
    }

    fn advance_animation(&mut self, queue: &Queue, now: Instant) -> bool {
        let Some(next_frame) = self.next_frame else {
            return false;
        };
        if now < next_frame {
            return false;
        }

        self.current_frame = (self.current_frame + 1) % self.frames.len();
        self.next_frame = Some(now + self.frames[self.current_frame].duration);
        self.upload_current_frame(queue);
        true
    }

    fn upload_current_frame(&self, queue: &Queue) {
        let frame = &self.frames[self.current_frame];
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            &frame.pixels,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.size.width * 4),
                rows_per_image: Some(self.size.height),
            },
            self.size,
        );
    }

    fn render<'a>(&'a self, pass: &mut RenderPass<'a>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

struct DecodedImage {
    size: Extent3d,
    frames: Vec<ImageFrame>,
}

struct ImageFrame {
    pixels: Vec<u8>,
    duration: Duration,
}

impl DecodedImage {
    fn load(path: &Path, opacity: f32, max_dimension: u32) -> Result<Self> {
        let is_gif = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("gif"));
        if is_gif {
            return Self::load_gif(path, opacity, max_dimension);
        }

        let mut reader = ImageReader::open(path)
            .with_context(|| format!("could not open background image {}", path.display()))?;
        reader.limits(image_limits(max_dimension));
        let image = reader
            .decode()
            .with_context(|| format!("could not decode background image {}", path.display()))?;
        Self::from_static_image(image, opacity)
    }

    fn load_gif(path: &Path, opacity: f32, max_dimension: u32) -> Result<Self> {
        let file = File::open(path)
            .with_context(|| format!("could not open background image {}", path.display()))?;
        let mut decoder = GifDecoder::new(BufReader::new(file))
            .with_context(|| format!("could not decode GIF background {}", path.display()))?;
        decoder
            .set_limits(image_limits(max_dimension))
            .with_context(|| format!("background GIF {} exceeds image limits", path.display()))?;

        let mut width = 0;
        let mut height = 0;
        let mut total_bytes = 0_u64;
        let mut decoded_frames = Vec::new();
        for frame in decoder.into_frames() {
            if decoded_frames.len() == MAX_GIF_FRAMES {
                bail!(
                    "background GIF {} exceeds the {MAX_GIF_FRAMES}-frame limit",
                    path.display()
                );
            }
            let frame = frame
                .with_context(|| format!("could not read GIF frames from {}", path.display()))?;
            let delay = frame_delay(frame.delay().numer_denom_ms());
            let buffer = frame.into_buffer();
            if decoded_frames.is_empty() {
                width = buffer.width();
                height = buffer.height();
                if width == 0 || height == 0 {
                    bail!("background GIF {} has an empty frame", path.display());
                }
            } else if buffer.width() != width || buffer.height() != height {
                bail!(
                    "background GIF {} has inconsistent frame dimensions",
                    path.display()
                );
            }
            let mut pixels = buffer.into_raw();
            total_bytes = total_bytes
                .checked_add(u64::try_from(pixels.len()).unwrap_or(u64::MAX))
                .filter(|total| *total <= MAX_GIF_BYTES)
                .context("background GIF exceeds the memory limit")?;
            premultiply_alpha(&mut pixels, opacity);
            decoded_frames.push(ImageFrame {
                pixels,
                duration: delay,
            });
        }

        if decoded_frames.is_empty() {
            bail!("background GIF {} contains no frames", path.display());
        }

        Ok(Self {
            size: Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            frames: decoded_frames,
        })
    }

    fn from_static_image(image: DynamicImage, opacity: f32) -> Result<Self> {
        let image = image.into_rgba8();
        let width = image.width();
        let height = image.height();
        if width == 0 || height == 0 {
            bail!("background image has no pixels");
        }
        let mut pixels = image.into_raw();
        premultiply_alpha(&mut pixels, opacity);

        Ok(Self {
            size: Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            frames: vec![ImageFrame {
                pixels,
                duration: Duration::ZERO,
            }],
        })
    }
}

fn image_limits(max_dimension: u32) -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(max_dimension);
    limits.max_image_height = Some(max_dimension);
    limits.max_alloc = Some(MAX_IMAGE_BYTES);
    limits
}

fn background_bind_group_layout(device: &Device) -> BindGroupLayout {
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("chymeral background bind group layout"),
        entries: &[
            BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 1,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Sampler(SamplerBindingType::Filtering),
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 2,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    })
}

fn background_pipeline(
    device: &Device,
    format: TextureFormat,
    bind_group_layout: &BindGroupLayout,
) -> RenderPipeline {
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("chymeral background shader"),
        source: ShaderSource::Wgsl(Cow::Borrowed(IMAGE_SHADER)),
    });
    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("chymeral background pipeline layout"),
        bind_group_layouts: &[Some(bind_group_layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("chymeral background pipeline"),
        layout: Some(&layout),
        vertex: VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: PipelineCompilationOptions::default(),
        },
        fragment: Some(FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(ColorTargetState {
                format,
                blend: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: ColorWrites::ALL,
            })],
            compilation_options: PipelineCompilationOptions::default(),
        }),
        primitive: PrimitiveState::default(),
        depth_stencil: None,
        multisample: MultisampleState::default(),
        cache: None,
        multiview_mask: None,
    })
}

fn image_transform(
    image_width: u32,
    image_height: u32,
    surface_width: u32,
    surface_height: u32,
    fit: ImageFit,
) -> [f32; 4] {
    if image_width == 0 || image_height == 0 || surface_width == 0 || surface_height == 0 {
        return [1.0, 1.0, 0.0, 0.0];
    }

    let image_aspect = image_width as f32 / image_height as f32;
    let surface_aspect = surface_width as f32 / surface_height as f32;

    match fit {
        ImageFit::Stretch => [1.0, 1.0, 0.0, 0.0],
        ImageFit::Cover if image_aspect > surface_aspect => {
            let width = surface_aspect / image_aspect;
            [width, 1.0, (1.0 - width) / 2.0, 0.0]
        }
        ImageFit::Cover => {
            let height = image_aspect / surface_aspect;
            [1.0, height, 0.0, (1.0 - height) / 2.0]
        }
        ImageFit::Contain if image_aspect > surface_aspect => {
            let height = surface_aspect / image_aspect;
            [1.0, 1.0 / height, 0.0, -(1.0 - height) / (2.0 * height)]
        }
        ImageFit::Contain => {
            let width = image_aspect / surface_aspect;
            [1.0 / width, 1.0, -(1.0 - width) / (2.0 * width), 0.0]
        }
    }
}

fn transform_bytes(transform: [f32; 4]) -> [u8; 16] {
    let mut bytes = [0_u8; 16];
    for (index, component) in transform.into_iter().enumerate() {
        bytes[index * 4..(index + 1) * 4].copy_from_slice(&component.to_ne_bytes());
    }
    bytes
}

fn premultiply_alpha(pixels: &mut [u8], opacity: f32) {
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let alpha = f32::from(pixel[3]) / 255.0 * opacity;
        pixel[0] = (f32::from(pixel[0]) * alpha).round() as u8;
        pixel[1] = (f32::from(pixel[1]) * alpha).round() as u8;
        pixel[2] = (f32::from(pixel[2]) * alpha).round() as u8;
        pixel[3] = (alpha * 255.0).round() as u8;
    }
}

fn frame_delay((numerator, denominator): (u32, u32)) -> Duration {
    if numerator == 0 || denominator == 0 {
        return Duration::from_millis(20);
    }

    Duration::from_secs_f64(f64::from(numerator) / f64::from(denominator) / 1_000.0)
        .max(Duration::from_millis(20))
}

#[cfg(test)]
mod tests {
    use std::{fs::File, path::Path, time::Duration};

    use image::{Delay, Frame, Rgba, RgbaImage, codecs::gif::GifEncoder};

    use super::{
        DecodedImage, ImageFit, MAX_IMAGE_DIMENSION, image_transform, load_static_texture,
        premultiply_alpha,
    };

    #[test]
    fn cover_crops_the_wider_axis() {
        let transform = image_transform(1600, 900, 800, 800, ImageFit::Cover);

        assert_eq!(transform, [0.5625, 1.0, 0.21875, 0.0]);
    }

    #[test]
    fn contain_leaves_transparent_bars() {
        let transform = image_transform(800, 800, 1600, 900, ImageFit::Contain);

        assert!((transform[0] - 16.0 / 9.0).abs() < f32::EPSILON);
        assert_eq!(transform[1], 1.0);
        assert!((transform[2] + 7.0 / 18.0).abs() < f32::EPSILON);
        assert_eq!(transform[3], 0.0);
    }

    #[test]
    fn multiplies_image_alpha_by_the_configured_opacity() {
        let mut pixels = [200, 100, 50, 128];
        premultiply_alpha(&mut pixels, 0.5);

        assert_eq!(pixels, [50, 25, 13, 64]);
    }

    #[test]
    fn decodes_all_gif_frames_for_animation() {
        let path =
            std::env::temp_dir().join(format!("chymeral-{}-animation.gif", std::process::id()));
        let first = Frame::from_parts(
            RgbaImage::from_pixel(2, 2, Rgba([255, 0, 0, 255])),
            0,
            0,
            Delay::from_numer_denom_ms(40, 1),
        );
        let second = Frame::from_parts(
            RgbaImage::from_pixel(2, 2, Rgba([0, 0, 255, 255])),
            0,
            0,
            Delay::from_numer_denom_ms(80, 1),
        );
        GifEncoder::new(File::create(&path).unwrap())
            .encode_frames([first, second])
            .unwrap();

        let decoded = DecodedImage::load(&path, 0.5, MAX_IMAGE_DIMENSION).unwrap();
        std::fs::remove_file(&path).unwrap();

        assert_eq!(decoded.frames.len(), 2);
        assert_eq!(decoded.frames[0].duration, Duration::from_millis(40));
        assert_eq!(decoded.frames[1].duration, Duration::from_millis(80));
        assert_eq!(&decoded.frames[0].pixels[..4], &[128, 0, 0, 128]);
        assert_eq!(&decoded.frames[1].pixels[..4], &[0, 0, 128, 128]);
    }

    #[test]
    fn loads_a_static_splash_without_premultiplying_its_alpha() {
        let path = std::env::temp_dir().join(format!("chymeral-{}-splash.png", std::process::id()));
        RgbaImage::from_pixel(2, 1, Rgba([200, 100, 50, 128]))
            .save(&path)
            .unwrap();

        let texture = load_static_texture(&path, MAX_IMAGE_DIMENSION).unwrap();
        std::fs::remove_file(&path).unwrap();

        assert_eq!(texture.width(), 2);
        assert_eq!(texture.height(), 1);
        assert_eq!(&texture.pixels()[..4], &[200, 100, 50, 128]);
    }

    #[test]
    fn rejects_animated_splash_extensions_before_opening_them() {
        assert!(load_static_texture(Path::new("splash.gif"), MAX_IMAGE_DIMENSION).is_err());
    }
}

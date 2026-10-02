use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
};

use anyhow::{Context, Result};
use chymeral::{
    resources::{Texture as ResourceTexture, TextureId, TextureStore},
    scene::{NodeContent, Rect, Scene, SceneLayer},
};
use wgpu::{
    AddressMode, BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout,
    BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource, BindingType, BlendState,
    Buffer, BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites, Device, Extent3d,
    FilterMode, FragmentState, MipmapFilterMode, MultisampleState, Origin3d,
    PipelineCompilationOptions, PipelineLayoutDescriptor, PrimitiveState, Queue, RenderPass,
    RenderPipeline, RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor,
    ShaderModuleDescriptor, ShaderSource, ShaderStages, SurfaceConfiguration,
    TexelCopyBufferLayout, TexelCopyTextureInfo, Texture as GpuTexture, TextureAspect,
    TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType, TextureUsages,
    TextureViewDescriptor, TextureViewDimension, VertexAttribute, VertexBufferLayout, VertexFormat,
    VertexState, VertexStepMode,
};

const VERTICES_PER_IMAGE: u32 = 6;
const IMAGE_VERTEX_STRIDE: u64 = 20;
const TERMINAL_SELECTION_COLOR: [u8; 4] = [74, 121, 184, 144];
const IMAGE_VERTEX_ATTRIBUTES: [VertexAttribute; 3] = [
    VertexAttribute {
        format: VertexFormat::Float32x2,
        offset: 0,
        shader_location: 0,
    },
    VertexAttribute {
        format: VertexFormat::Float32x2,
        offset: 8,
        shader_location: 1,
    },
    VertexAttribute {
        format: VertexFormat::Float32,
        offset: 16,
        shader_location: 2,
    },
];

const IMAGE_SHADER: &str = r#"
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) opacity: f32,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) opacity: f32,
}

@group(0) @binding(0)
var image_texture: texture_2d<f32>;

@group(0) @binding(1)
var image_sampler: sampler;

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4<f32>(input.position, 0.0, 1.0);
    output.uv = input.uv;
    output.opacity = input.opacity;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let color = textureSample(image_texture, image_sampler, input.uv);
    let alpha = color.a * input.opacity;
    return vec4<f32>(color.rgb * alpha, alpha);
}
"#;

struct GpuImage {
    _texture: GpuTexture,
    bind_group: BindGroup,
}

#[derive(Clone, Copy)]
struct Draw {
    source: DrawSource,
    first_vertex: u32,
    layer: SceneLayer,
    scissor: ScissorRect,
}

#[derive(Clone, Copy)]
struct TerminalSelectionDraw {
    first_vertex: u32,
    scissor: ScissorRect,
}

#[derive(Clone, Copy)]
enum DrawSource {
    Texture(TextureId),
    Solid([u8; 4]),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScissorRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy)]
pub struct ImageViewport {
    width: u32,
    height: u32,
    scale_factor: f64,
}

impl ImageViewport {
    pub const fn new(width: u32, height: u32, scale_factor: f64) -> Self {
        Self {
            width,
            height,
            scale_factor,
        }
    }
}

pub struct ImageRenderer {
    bind_group_layout: BindGroupLayout,
    sampler: Sampler,
    pipeline: RenderPipeline,
    vertex_buffer: Buffer,
    vertex_capacity: u64,
    terminal_selection_vertex_buffer: Buffer,
    terminal_selection_vertex_capacity: u64,
    terminal_selection_draws: Vec<TerminalSelectionDraw>,
    gpu_images: HashMap<TextureId, GpuImage>,
    gpu_solids: HashMap<[u8; 4], GpuImage>,
    draws: Vec<Draw>,
}

impl ImageRenderer {
    pub fn new(device: &Device, surface_config: &SurfaceConfiguration) -> Self {
        let bind_group_layout = image_bind_group_layout(device);
        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("chymeral image sampler"),
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            address_mode_w: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let pipeline = image_pipeline(device, surface_config.format, &bind_group_layout);
        let vertex_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("chymeral image vertices"),
            size: IMAGE_VERTEX_STRIDE,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let terminal_selection_vertex_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("chymeral terminal selection vertices"),
            size: IMAGE_VERTEX_STRIDE,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            bind_group_layout,
            sampler,
            pipeline,
            vertex_buffer,
            vertex_capacity: IMAGE_VERTEX_STRIDE,
            terminal_selection_vertex_buffer,
            terminal_selection_vertex_capacity: IMAGE_VERTEX_STRIDE,
            terminal_selection_draws: Vec::new(),
            gpu_images: HashMap::new(),
            gpu_solids: HashMap::new(),
            draws: Vec::new(),
        }
    }

    pub fn prepare(
        &mut self,
        device: &Device,
        queue: &Queue,
        scene: &Scene,
        textures: &TextureStore,
        viewport: ImageViewport,
    ) -> Result<()> {
        self.gpu_images
            .retain(|texture, _| textures.texture(*texture).is_some());

        let paint_order = scene.paint_order();
        let solid_colors = paint_order
            .iter()
            .filter_map(|node_id| scene.node(*node_id))
            .filter_map(|node| match node.content() {
                NodeContent::Solid(solid) => Some(solid.color()),
                NodeContent::Empty | NodeContent::Image(_) => None,
            })
            .collect::<HashSet<_>>();
        self.gpu_solids
            .retain(|color, _| solid_colors.contains(color));

        let mut vertices = Vec::new();
        self.draws.clear();
        for node_id in paint_order {
            let Some(node) = scene.node(node_id) else {
                continue;
            };
            let (source, opacity) = match node.content() {
                NodeContent::Empty => continue,
                NodeContent::Image(image) => {
                    let Some(texture) = textures.texture(image.texture()) else {
                        continue;
                    };
                    self.ensure_gpu_image(device, queue, image.texture(), texture)?;
                    (DrawSource::Texture(image.texture()), image.opacity())
                }
                NodeContent::Solid(solid) => {
                    self.ensure_gpu_solid(device, queue, solid.color())?;
                    (DrawSource::Solid(solid.color()), 1.0)
                }
            };
            let bounds = scene.world_bounds(node_id)?;
            let Some(visible_bounds) = scene.visible_bounds(node_id)? else {
                continue;
            };
            let Some(scissor) = scissor_rect(visible_bounds, viewport) else {
                continue;
            };
            let Some(quad) = image_vertices(
                bounds,
                opacity,
                viewport.width,
                viewport.height,
                viewport.scale_factor,
            ) else {
                continue;
            };
            let first_vertex = u32::try_from(self.draws.len())
                .ok()
                .and_then(|count| count.checked_mul(VERTICES_PER_IMAGE))
                .context("too many image nodes to render")?;
            for vertex in quad {
                append_vertex(&mut vertices, vertex);
            }
            self.draws.push(Draw {
                source,
                first_vertex,
                layer: node.layer(),
                scissor,
            });
        }

        self.ensure_vertex_capacity(device, vertices.len() as u64);
        if !vertices.is_empty() {
            queue.write_buffer(&self.vertex_buffer, 0, &vertices);
        }
        Ok(())
    }

    pub fn prepare_terminal_selection(
        &mut self,
        device: &Device,
        queue: &Queue,
        rectangles: &[Rect],
        viewport: ImageViewport,
    ) -> Result<()> {
        self.terminal_selection_draws.clear();
        if rectangles.is_empty() {
            return Ok(());
        }

        self.ensure_gpu_solid(device, queue, TERMINAL_SELECTION_COLOR)?;
        let mut vertices = Vec::new();
        for rectangle in rectangles {
            let Some(scissor) = scissor_rect(*rectangle, viewport) else {
                continue;
            };
            let Some(quad) = image_vertices(
                *rectangle,
                1.0,
                viewport.width,
                viewport.height,
                viewport.scale_factor,
            ) else {
                continue;
            };
            let first_vertex = u32::try_from(self.terminal_selection_draws.len())
                .ok()
                .and_then(|count| count.checked_mul(VERTICES_PER_IMAGE))
                .context("too many terminal selection ranges to render")?;
            for vertex in quad {
                append_vertex(&mut vertices, vertex);
            }
            self.terminal_selection_draws.push(TerminalSelectionDraw {
                first_vertex,
                scissor,
            });
        }

        self.ensure_terminal_selection_vertex_capacity(device, vertices.len() as u64);
        if !vertices.is_empty() {
            queue.write_buffer(&self.terminal_selection_vertex_buffer, 0, &vertices);
        }
        Ok(())
    }

    pub fn render_layer<'a>(&'a self, pass: &mut RenderPass<'a>, layer: SceneLayer) {
        if !self.draws.iter().any(|draw| draw.layer == layer) {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        for draw in self.draws.iter().filter(|draw| draw.layer == layer) {
            let image = match draw.source {
                DrawSource::Texture(texture) => self.gpu_images.get(&texture),
                DrawSource::Solid(color) => self.gpu_solids.get(&color),
            };
            let Some(image) = image else {
                continue;
            };
            pass.set_bind_group(0, &image.bind_group, &[]);
            pass.set_scissor_rect(
                draw.scissor.x,
                draw.scissor.y,
                draw.scissor.width,
                draw.scissor.height,
            );
            pass.draw(
                draw.first_vertex..draw.first_vertex + VERTICES_PER_IMAGE,
                0..1,
            );
        }
    }

    pub fn render_terminal_selection<'a>(&'a self, pass: &mut RenderPass<'a>) {
        if self.terminal_selection_draws.is_empty() {
            return;
        }
        let Some(image) = self.gpu_solids.get(&TERMINAL_SELECTION_COLOR) else {
            return;
        };
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.terminal_selection_vertex_buffer.slice(..));
        pass.set_bind_group(0, &image.bind_group, &[]);
        for draw in &self.terminal_selection_draws {
            pass.set_scissor_rect(
                draw.scissor.x,
                draw.scissor.y,
                draw.scissor.width,
                draw.scissor.height,
            );
            pass.draw(
                draw.first_vertex..draw.first_vertex + VERTICES_PER_IMAGE,
                0..1,
            );
        }
    }

    fn ensure_gpu_image(
        &mut self,
        device: &Device,
        queue: &Queue,
        texture_id: TextureId,
        texture: &ResourceTexture,
    ) -> Result<()> {
        if self.gpu_images.contains_key(&texture_id) {
            return Ok(());
        }
        let image = self.create_gpu_image(
            device,
            queue,
            texture.width(),
            texture.height(),
            texture.pixels(),
        )?;
        self.gpu_images.insert(texture_id, image);
        Ok(())
    }

    fn ensure_gpu_solid(&mut self, device: &Device, queue: &Queue, color: [u8; 4]) -> Result<()> {
        if self.gpu_solids.contains_key(&color) {
            return Ok(());
        }
        let image = self.create_gpu_image(device, queue, 1, 1, &color)?;
        self.gpu_solids.insert(color, image);
        Ok(())
    }

    fn create_gpu_image(
        &self,
        device: &Device,
        queue: &Queue,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<GpuImage> {
        let bytes_per_row = width
            .checked_mul(4)
            .context("texture row size overflows wgpu's upload layout")?;
        let gpu_texture = device.create_texture(&TextureDescriptor {
            label: Some("chymeral scene image"),
            size: Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &gpu_texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            pixels,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
            Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let texture_view = gpu_texture.create_view(&TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("chymeral scene image bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&texture_view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        Ok(GpuImage {
            _texture: gpu_texture,
            bind_group,
        })
    }

    fn ensure_vertex_capacity(&mut self, device: &Device, required: u64) {
        if required <= self.vertex_capacity {
            return;
        }
        let size = required.checked_next_power_of_two().unwrap_or(required);
        self.vertex_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("chymeral image vertices"),
            size,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.vertex_capacity = size;
    }

    fn ensure_terminal_selection_vertex_capacity(&mut self, device: &Device, required: u64) {
        if required <= self.terminal_selection_vertex_capacity {
            return;
        }
        let size = required.checked_next_power_of_two().unwrap_or(required);
        self.terminal_selection_vertex_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("chymeral terminal selection vertices"),
            size,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.terminal_selection_vertex_capacity = size;
    }
}

fn image_bind_group_layout(device: &Device) -> BindGroupLayout {
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("chymeral scene image bind group layout"),
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
        ],
    })
}

fn image_pipeline(
    device: &Device,
    format: TextureFormat,
    bind_group_layout: &BindGroupLayout,
) -> RenderPipeline {
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("chymeral scene image shader"),
        source: ShaderSource::Wgsl(Cow::Borrowed(IMAGE_SHADER)),
    });
    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("chymeral scene image pipeline layout"),
        bind_group_layouts: &[Some(bind_group_layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("chymeral scene image pipeline"),
        layout: Some(&layout),
        vertex: VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[Some(VertexBufferLayout {
                array_stride: IMAGE_VERTEX_STRIDE,
                step_mode: VertexStepMode::Vertex,
                attributes: &IMAGE_VERTEX_ATTRIBUTES,
            })],
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

fn image_vertices(
    bounds: Rect,
    opacity: f32,
    surface_width: u32,
    surface_height: u32,
    scale_factor: f64,
) -> Option<[[f32; 5]; 6]> {
    if surface_width == 0 || surface_height == 0 || !scale_factor.is_finite() || scale_factor <= 0.0
    {
        return None;
    }
    let left = bounds.x() * scale_factor;
    let right = (bounds.x() + bounds.width()) * scale_factor;
    let top = bounds.y() * scale_factor;
    let bottom = (bounds.y() + bounds.height()) * scale_factor;
    if !left.is_finite() || !right.is_finite() || !top.is_finite() || !bottom.is_finite() {
        return None;
    }
    let to_x = |value: f64| clip_coordinate(value, surface_width, false);
    let to_y = |value: f64| clip_coordinate(value, surface_height, true);
    let (Some(left_x), Some(right_x), Some(top_y), Some(bottom_y)) =
        (to_x(left), to_x(right), to_y(top), to_y(bottom))
    else {
        return None;
    };
    let top_left = [left_x, top_y];
    let top_right = [right_x, top_y];
    let bottom_left = [left_x, bottom_y];
    let bottom_right = [right_x, bottom_y];
    Some([
        [top_left[0], top_left[1], 0.0, 0.0, opacity],
        [top_right[0], top_right[1], 1.0, 0.0, opacity],
        [bottom_right[0], bottom_right[1], 1.0, 1.0, opacity],
        [top_left[0], top_left[1], 0.0, 0.0, opacity],
        [bottom_right[0], bottom_right[1], 1.0, 1.0, opacity],
        [bottom_left[0], bottom_left[1], 0.0, 1.0, opacity],
    ])
}

fn scissor_rect(bounds: Rect, viewport: ImageViewport) -> Option<ScissorRect> {
    if viewport.width == 0
        || viewport.height == 0
        || !viewport.scale_factor.is_finite()
        || viewport.scale_factor <= 0.0
    {
        return None;
    }
    let left = (bounds.x() * viewport.scale_factor).floor();
    let top = (bounds.y() * viewport.scale_factor).floor();
    let right = ((bounds.x() + bounds.width()) * viewport.scale_factor).ceil();
    let bottom = ((bounds.y() + bounds.height()) * viewport.scale_factor).ceil();
    if !left.is_finite() || !top.is_finite() || !right.is_finite() || !bottom.is_finite() {
        return None;
    }
    let left = left.clamp(0.0, f64::from(viewport.width));
    let top = top.clamp(0.0, f64::from(viewport.height));
    let right = right.clamp(0.0, f64::from(viewport.width));
    let bottom = bottom.clamp(0.0, f64::from(viewport.height));
    if right <= left || bottom <= top {
        return None;
    }
    Some(ScissorRect {
        x: left as u32,
        y: top as u32,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    })
}

fn clip_coordinate(value: f64, length: u32, inverted: bool) -> Option<f32> {
    let ratio = value / f64::from(length) * 2.0;
    let coordinate = if inverted { 1.0 - ratio } else { ratio - 1.0 };
    (coordinate.is_finite() && coordinate.abs() <= f64::from(f32::MAX)).then_some(coordinate as f32)
}

fn append_vertex(bytes: &mut Vec<u8>, vertex: [f32; 5]) {
    for component in vertex {
        bytes.extend_from_slice(&component.to_ne_bytes());
    }
}

#[cfg(test)]
mod tests {
    use chymeral::scene::Rect;

    use super::{ImageViewport, ScissorRect, image_vertices, scissor_rect};

    #[test]
    fn maps_logical_bounds_to_clip_space() {
        let vertices = image_vertices(
            Rect::new(10.0, 20.0, 30.0, 40.0).unwrap(),
            0.5,
            100,
            100,
            1.0,
        )
        .unwrap();

        assert_eq!(vertices[0], [-0.8, 0.6, 0.0, 0.0, 0.5]);
        assert_eq!(vertices[2], [-0.2, -0.2, 1.0, 1.0, 0.5]);
    }

    #[test]
    fn skips_bounds_that_cannot_be_represented_by_gpu_vertices() {
        assert!(
            image_vertices(Rect::new(1e42, 0.0, 1e41, 1.0).unwrap(), 1.0, 100, 100, 1.0,).is_none()
        );
    }

    #[test]
    fn converts_visible_logical_bounds_to_a_clamped_physical_scissor() {
        assert_eq!(
            scissor_rect(
                Rect::new(-1.1, 2.25, 4.4, 3.5).unwrap(),
                ImageViewport::new(20, 20, 2.0),
            ),
            Some(ScissorRect {
                x: 0,
                y: 4,
                width: 7,
                height: 8,
            })
        );
        assert_eq!(
            scissor_rect(
                Rect::new(20.0, 0.0, 1.0, 1.0).unwrap(),
                ImageViewport::new(20, 20, 1.0),
            ),
            None
        );
    }
}

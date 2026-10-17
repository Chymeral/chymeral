use std::{
    error::Error,
    fmt::{Display, Formatter},
    sync::Arc,
};

use crate::{animated_image::AnimatedImageId, resources::TextureId, scene_text::TextNode};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId {
    index: usize,
    generation: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SceneLayer {
    Underlay,
    #[default]
    Overlay,
}

impl SceneLayer {
    pub const PAINT_ORDER: [Self; 2] = [Self::Underlay, Self::Overlay];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryError {
    NonFiniteCoordinate,
    NegativeExtent,
    ExtentOverflow,
}

impl Display for GeometryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonFiniteCoordinate => formatter.write_str("geometry coordinates must be finite"),
            Self::NegativeExtent => formatter.write_str("rectangle extents cannot be negative"),
            Self::ExtentOverflow => {
                formatter.write_str("rectangle origin and extent must produce finite bounds")
            }
        }
    }
}

impl Error for GeometryError {}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    x: f64,
    y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Result<Self, GeometryError> {
        if !x.is_finite() || !y.is_finite() {
            return Err(GeometryError::NonFiniteCoordinate);
        }
        Ok(Self { x, y })
    }

    pub const fn x(self) -> f64 {
        self.x
    }

    pub const fn y(self) -> f64 {
        self.y
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Transform {
    x: f64,
    y: f64,
}

impl Transform {
    pub fn new(x: f64, y: f64) -> Result<Self, GeometryError> {
        if !x.is_finite() || !y.is_finite() {
            return Err(GeometryError::NonFiniteCoordinate);
        }
        Ok(Self { x, y })
    }

    pub const fn x(self) -> f64 {
        self.x
    }

    pub const fn y(self) -> f64 {
        self.y
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Result<Self, GeometryError> {
        if !x.is_finite() || !y.is_finite() || !width.is_finite() || !height.is_finite() {
            return Err(GeometryError::NonFiniteCoordinate);
        }
        if width < 0.0 || height < 0.0 {
            return Err(GeometryError::NegativeExtent);
        }
        let right = x + width;
        let bottom = y + height;
        if !right.is_finite()
            || !bottom.is_finite()
            || (width > 0.0 && right <= x)
            || (height > 0.0 && bottom <= y)
        {
            return Err(GeometryError::ExtentOverflow);
        }
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }

    pub const fn x(self) -> f64 {
        self.x
    }

    pub const fn y(self) -> f64 {
        self.y
    }

    pub const fn width(self) -> f64 {
        self.width
    }

    pub const fn height(self) -> f64 {
        self.height
    }

    pub const fn is_empty(self) -> bool {
        self.width == 0.0 || self.height == 0.0
    }

    pub fn contains(self, point: Point) -> bool {
        point.x >= self.x && point.x < self.right() && point.y >= self.y && point.y < self.bottom()
    }

    pub fn intersects(self, other: Self) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && self.x < other.right()
            && self.right() > other.x
            && self.y < other.bottom()
            && self.bottom() > other.y
    }

    pub fn intersection(self, other: Self) -> Option<Self> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        (right > x && bottom > y)
            .then(|| Self::new(x, y, right - x, bottom - y).expect("finite rectangle intersection"))
    }

    fn right(self) -> f64 {
        self.x + self.width
    }

    fn bottom(self) -> f64 {
        self.y + self.height
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeProperties {
    pub transform: Transform,
    pub bounds: Rect,
    pub visible: bool,
    pub clip_children: bool,
    pub z_index: i32,
    pub overlap_enabled: bool,
    pub external_drop_enabled: bool,
    pub input_passthrough: bool,
}

impl Default for NodeProperties {
    fn default() -> Self {
        Self {
            transform: Transform::default(),
            bounds: Rect::default(),
            visible: true,
            clip_children: false,
            z_index: 0,
            overlap_enabled: false,
            external_drop_enabled: false,
            input_passthrough: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImageContactShape {
    #[default]
    Bounds,
    Alpha {
        threshold: u8,
    },
}

impl ImageContactShape {
    pub const fn alpha() -> Self {
        Self::Alpha { threshold: 1 }
    }

    const fn threshold(self) -> Option<u8> {
        match self {
            Self::Bounds => None,
            Self::Alpha { threshold } => Some(threshold),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImageAlphaMask {
    width: u32,
    height: u32,
    alpha: Arc<[u8]>,
}

impl ImageAlphaMask {
    pub(crate) fn new(width: u32, height: u32, alpha: Arc<[u8]>) -> Result<Self, ImageNodeError> {
        let expected = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .ok_or(ImageNodeError::InvalidAlphaMask)?;
        if expected == 0 || alpha.len() != expected {
            return Err(ImageNodeError::InvalidAlphaMask);
        }
        Ok(Self {
            width,
            height,
            alpha,
        })
    }

    fn contains(&self, bounds: Rect, point: Point, threshold: u8) -> bool {
        if !bounds.contains(point) {
            return false;
        }
        let x = ((point.x() - bounds.x()) * f64::from(self.width) / bounds.width()).floor();
        let y = ((point.y() - bounds.y()) * f64::from(self.height) / bounds.height()).floor();
        let x = x.clamp(0.0, f64::from(self.width.saturating_sub(1))) as usize;
        let y = y.clamp(0.0, f64::from(self.height.saturating_sub(1))) as usize;
        self.alpha[y * self.width as usize + x] >= threshold
    }

    fn intersects_rect(&self, bounds: Rect, area: Rect, threshold: u8) -> bool {
        let Some(area) = bounds.intersection(area) else {
            return false;
        };
        let Some((start_x, end_x, start_y, end_y)) = self.pixel_range(bounds, area) else {
            return false;
        };
        for y in start_y..end_y {
            let row = y * self.width as usize;
            if self.alpha[row + start_x..row + end_x]
                .iter()
                .any(|alpha| *alpha >= threshold)
            {
                return true;
            }
        }
        false
    }

    fn pixel_range(&self, bounds: Rect, area: Rect) -> Option<(usize, usize, usize, usize)> {
        let area = bounds.intersection(area)?;
        let width = self.width as usize;
        let height = self.height as usize;
        let start_x = ((area.x() - bounds.x()) * f64::from(self.width) / bounds.width())
            .floor()
            .clamp(0.0, self.width as f64) as usize;
        let end_x = ((area.right() - bounds.x()) * f64::from(self.width) / bounds.width())
            .ceil()
            .clamp(0.0, self.width as f64) as usize;
        let start_y = ((area.y() - bounds.y()) * f64::from(self.height) / bounds.height())
            .floor()
            .clamp(0.0, self.height as f64) as usize;
        let end_y = ((area.bottom() - bounds.y()) * f64::from(self.height) / bounds.height())
            .ceil()
            .clamp(0.0, self.height as f64) as usize;
        (start_x < end_x && end_x <= width && start_y < end_y && end_y <= height)
            .then_some((start_x, end_x, start_y, end_y))
    }

    fn pixel_count_in(&self, bounds: Rect, area: Rect) -> usize {
        self.pixel_range(bounds, area)
            .map(|(start_x, end_x, start_y, end_y)| {
                (end_x - start_x).saturating_mul(end_y - start_y)
            })
            .unwrap_or_default()
    }

    fn opaque_pixels_in(
        &self,
        bounds: Rect,
        area: Rect,
        threshold: u8,
    ) -> impl Iterator<Item = Rect> + '_ {
        let range = self.pixel_range(bounds, area);
        let width = self.width as usize;
        range
            .into_iter()
            .flat_map(move |(start_x, end_x, start_y, end_y)| {
                (start_y..end_y).flat_map(move |y| {
                    (start_x..end_x)
                        .filter(move |x| self.alpha[y * width + *x] >= threshold)
                        .map(move |x| {
                            Rect::new(
                                bounds.x() + x as f64 * bounds.width() / f64::from(self.width),
                                bounds.y() + y as f64 * bounds.height() / f64::from(self.height),
                                bounds.width() / f64::from(self.width),
                                bounds.height() / f64::from(self.height),
                            )
                            .expect("image alpha cells retain finite non-negative geometry")
                        })
                })
            })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageNode {
    texture: TextureId,
    opacity: f32,
    contact_shape: ImageContactShape,
    alpha_mask: Option<ImageAlphaMask>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnimatedImageNode {
    image: AnimatedImageId,
    opacity: f32,
    contact_shape: ImageContactShape,
    alpha_mask: Option<ImageAlphaMask>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SolidNode {
    color: [u8; 4],
}

impl SolidNode {
    pub const fn new(color: [u8; 4]) -> Self {
        Self { color }
    }

    pub const fn color(self) -> [u8; 4] {
        self.color
    }
}

impl ImageNode {
    pub fn new(texture: TextureId, opacity: f32) -> Result<Self, ImageNodeError> {
        Self::with_contact_shape(texture, opacity, ImageContactShape::Bounds, None)
    }

    pub(crate) fn with_contact_shape(
        texture: TextureId,
        opacity: f32,
        contact_shape: ImageContactShape,
        alpha_mask: Option<ImageAlphaMask>,
    ) -> Result<Self, ImageNodeError> {
        if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) {
            return Err(ImageNodeError::InvalidOpacity);
        }
        validate_contact_shape(contact_shape, alpha_mask.as_ref())?;
        Ok(Self {
            texture,
            opacity,
            contact_shape,
            alpha_mask,
        })
    }

    pub const fn texture(&self) -> TextureId {
        self.texture
    }

    pub const fn opacity(&self) -> f32 {
        self.opacity
    }

    pub const fn contact_shape(&self) -> ImageContactShape {
        self.contact_shape
    }

    fn contains(&self, bounds: Rect, point: Point) -> bool {
        match (self.contact_shape.threshold(), self.alpha_mask.as_ref()) {
            (Some(threshold), Some(alpha_mask)) => alpha_mask.contains(bounds, point, threshold),
            (Some(_), None) => false,
            (None, _) => bounds.contains(point),
        }
    }

    fn intersects_rect(&self, bounds: Rect, area: Rect) -> bool {
        match (self.contact_shape.threshold(), self.alpha_mask.as_ref()) {
            (Some(threshold), Some(alpha_mask)) => {
                alpha_mask.intersects_rect(bounds, area, threshold)
            }
            (Some(_), None) => false,
            (None, _) => bounds.intersects(area),
        }
    }
}

impl AnimatedImageNode {
    pub fn new(image: AnimatedImageId, opacity: f32) -> Result<Self, ImageNodeError> {
        Self::with_contact_shape(image, opacity, ImageContactShape::Bounds, None)
    }

    pub(crate) fn with_contact_shape(
        image: AnimatedImageId,
        opacity: f32,
        contact_shape: ImageContactShape,
        alpha_mask: Option<ImageAlphaMask>,
    ) -> Result<Self, ImageNodeError> {
        if !opacity.is_finite() || !(0.0..=1.0).contains(&opacity) {
            return Err(ImageNodeError::InvalidOpacity);
        }
        validate_contact_shape(contact_shape, alpha_mask.as_ref())?;
        Ok(Self {
            image,
            opacity,
            contact_shape,
            alpha_mask,
        })
    }

    pub const fn image(&self) -> AnimatedImageId {
        self.image
    }

    pub const fn opacity(&self) -> f32 {
        self.opacity
    }

    pub const fn contact_shape(&self) -> ImageContactShape {
        self.contact_shape
    }

    pub(crate) fn with_alpha_mask(&self, alpha_mask: ImageAlphaMask) -> Self {
        Self {
            image: self.image,
            opacity: self.opacity,
            contact_shape: self.contact_shape,
            alpha_mask: Some(alpha_mask),
        }
    }

    pub(crate) fn alpha_mask(&self) -> Option<&ImageAlphaMask> {
        self.alpha_mask.as_ref()
    }

    fn contains(&self, bounds: Rect, point: Point) -> bool {
        match (self.contact_shape.threshold(), self.alpha_mask.as_ref()) {
            (Some(threshold), Some(alpha_mask)) => alpha_mask.contains(bounds, point, threshold),
            (Some(_), None) => false,
            (None, _) => bounds.contains(point),
        }
    }

    fn intersects_rect(&self, bounds: Rect, area: Rect) -> bool {
        match (self.contact_shape.threshold(), self.alpha_mask.as_ref()) {
            (Some(threshold), Some(alpha_mask)) => {
                alpha_mask.intersects_rect(bounds, area, threshold)
            }
            (Some(_), None) => false,
            (None, _) => bounds.intersects(area),
        }
    }
}

fn validate_contact_shape(
    contact_shape: ImageContactShape,
    alpha_mask: Option<&ImageAlphaMask>,
) -> Result<(), ImageNodeError> {
    match contact_shape {
        ImageContactShape::Bounds => Ok(()),
        ImageContactShape::Alpha { threshold: 0 } => Err(ImageNodeError::InvalidAlphaThreshold),
        ImageContactShape::Alpha { .. } if alpha_mask.is_none() => {
            Err(ImageNodeError::MissingAlphaMask)
        }
        ImageContactShape::Alpha { .. } => Ok(()),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageNodeError {
    InvalidOpacity,
    InvalidAlphaThreshold,
    MissingAlphaMask,
    InvalidAlphaMask,
}

impl Display for ImageNodeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidOpacity => {
                formatter.write_str("image opacity must be between 0.0 and 1.0")
            }
            Self::InvalidAlphaThreshold => {
                formatter.write_str("image alpha contact threshold must be between 1 and 255")
            }
            Self::MissingAlphaMask => {
                formatter.write_str("image alpha contact requires source alpha data")
            }
            Self::InvalidAlphaMask => {
                formatter.write_str("image alpha data does not match its source dimensions")
            }
        }
    }
}

impl Error for ImageNodeError {}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum NodeContent {
    #[default]
    Empty,
    Solid(SolidNode),
    Image(ImageNode),
    AnimatedImage(AnimatedImageNode),
    Text(TextNode),
}

impl NodeContent {
    fn contains(&self, bounds: Rect, point: Point) -> bool {
        match self {
            Self::Image(image) => image.contains(bounds, point),
            Self::AnimatedImage(image) => image.contains(bounds, point),
            Self::Empty | Self::Solid(_) | Self::Text(_) => bounds.contains(point),
        }
    }

    fn intersects_rect(&self, bounds: Rect, area: Rect) -> bool {
        match self {
            Self::Image(image) => image.intersects_rect(bounds, area),
            Self::AnimatedImage(image) => image.intersects_rect(bounds, area),
            Self::Empty | Self::Solid(_) | Self::Text(_) => bounds.intersects(area),
        }
    }

    fn alpha_mask_pixel_count_in(&self, bounds: Rect, area: Rect) -> Option<usize> {
        match self {
            Self::Image(ImageNode {
                contact_shape: ImageContactShape::Alpha { .. },
                alpha_mask: Some(mask),
                ..
            })
            | Self::AnimatedImage(AnimatedImageNode {
                contact_shape: ImageContactShape::Alpha { .. },
                alpha_mask: Some(mask),
                ..
            }) => Some(mask.pixel_count_in(bounds, area)),
            Self::Empty
            | Self::Solid(_)
            | Self::Image(_)
            | Self::AnimatedImage(_)
            | Self::Text(_) => None,
        }
    }

    fn opaque_pixels_in(
        &self,
        bounds: Rect,
        area: Rect,
    ) -> Option<impl Iterator<Item = Rect> + '_> {
        match self {
            Self::Image(ImageNode {
                contact_shape: ImageContactShape::Alpha { threshold },
                alpha_mask: Some(mask),
                ..
            })
            | Self::AnimatedImage(AnimatedImageNode {
                contact_shape: ImageContactShape::Alpha { threshold },
                alpha_mask: Some(mask),
                ..
            }) => Some(mask.opaque_pixels_in(bounds, area, *threshold)),
            Self::Empty
            | Self::Solid(_)
            | Self::Image(_)
            | Self::AnimatedImage(_)
            | Self::Text(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    layer: SceneLayer,
    properties: NodeProperties,
    content: NodeContent,
}

impl Node {
    pub const fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    pub fn children(&self) -> &[NodeId] {
        &self.children
    }

    pub const fn layer(&self) -> SceneLayer {
        self.layer
    }

    pub const fn properties(&self) -> NodeProperties {
        self.properties
    }

    pub fn content(&self) -> &NodeContent {
        &self.content
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SceneError {
    InvalidNode(NodeId),
    WouldCreateCycle { node: NodeId, parent: NodeId },
    LayerRequiresRoot(NodeId),
    WorldBoundsOverflow(NodeId),
}

impl Display for SceneError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidNode(_) => formatter.write_str("node handle is no longer valid"),
            Self::WouldCreateCycle { .. } => {
                formatter.write_str("a scene node cannot parent one of its descendants")
            }
            Self::LayerRequiresRoot(_) => {
                formatter.write_str("only a scene root can change its layer")
            }
            Self::WorldBoundsOverflow(_) => {
                formatter.write_str("node transforms produce non-finite world bounds")
            }
        }
    }
}

impl Error for SceneError {}

#[derive(Debug)]
struct Slot {
    generation: u64,
    node: Option<Node>,
}

#[derive(Debug, Default)]
pub struct Scene {
    slots: Vec<Slot>,
    free_slots: Vec<usize>,
    roots: Vec<NodeId>,
    node_count: usize,
}

impl Scene {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.node_count
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn create_root(&mut self, properties: NodeProperties) -> NodeId {
        self.create_root_in_layer(properties, SceneLayer::Overlay)
    }

    pub fn create_root_in_layer(
        &mut self,
        properties: NodeProperties,
        layer: SceneLayer,
    ) -> NodeId {
        let node = self.insert_node(None, properties, layer);
        self.roots.push(node);
        self.node_count += 1;
        node
    }

    pub fn create_child(
        &mut self,
        parent: NodeId,
        properties: NodeProperties,
    ) -> Result<NodeId, SceneError> {
        let layer = self.require_node(parent)?.layer;
        let child = self.insert_node(Some(parent), properties, layer);
        self.require_node_mut(parent)?.children.push(child);
        self.node_count += 1;
        Ok(child)
    }

    pub fn node(&self, node: NodeId) -> Option<&Node> {
        let slot = self.slots.get(node.index)?;
        (slot.generation == node.generation)
            .then_some(slot.node.as_ref())
            .flatten()
    }

    pub fn roots(&self) -> &[NodeId] {
        &self.roots
    }

    pub fn set_properties(
        &mut self,
        node: NodeId,
        properties: NodeProperties,
    ) -> Result<(), SceneError> {
        self.require_node_mut(node)?.properties = properties;
        Ok(())
    }

    pub fn set_content(&mut self, node: NodeId, content: NodeContent) -> Result<(), SceneError> {
        self.require_node_mut(node)?.content = content;
        Ok(())
    }

    pub fn clear_content(&mut self, node: NodeId) -> Result<(), SceneError> {
        self.set_content(node, NodeContent::Empty)
    }

    pub fn references_texture(&self, texture: TextureId) -> bool {
        self.slots.iter().any(|slot| {
            matches!(
                slot.node.as_ref().map(Node::content),
                Some(NodeContent::Image(image)) if image.texture() == texture
            )
        })
    }

    pub fn references_animated_image(&self, image: AnimatedImageId) -> bool {
        self.slots.iter().any(|slot| {
            matches!(
                slot.node.as_ref().map(Node::content),
                Some(NodeContent::AnimatedImage(animated)) if animated.image() == image
            )
        })
    }

    pub fn set_root_layer(&mut self, node: NodeId, layer: SceneLayer) -> Result<(), SceneError> {
        if self.require_node(node)?.parent.is_some() {
            return Err(SceneError::LayerRequiresRoot(node));
        }
        self.set_subtree_layer(node, layer)
    }

    pub fn world_bounds(&self, node: NodeId) -> Result<Rect, SceneError> {
        let bounds = self.require_node(node)?.properties.bounds;
        let mut x = bounds.x();
        let mut y = bounds.y();
        let mut current = Some(node);
        while let Some(current_node) = current {
            let current_node_ref = self.require_node(current_node)?;
            x += current_node_ref.properties.transform.x();
            y += current_node_ref.properties.transform.y();
            if !x.is_finite() || !y.is_finite() {
                return Err(SceneError::WorldBoundsOverflow(node));
            }
            current = current_node_ref.parent;
        }
        Rect::new(x, y, bounds.width(), bounds.height())
            .map_err(|_| SceneError::WorldBoundsOverflow(node))
    }

    pub fn visible_bounds(&self, node: NodeId) -> Result<Option<Rect>, SceneError> {
        let node_ref = self.require_node(node)?;
        if !node_ref.properties.visible {
            return Ok(None);
        }

        let mut visible = self.world_bounds(node)?;
        let mut current = node_ref.parent;
        while let Some(current_node) = current {
            let current_ref = self.require_node(current_node)?;
            if !current_ref.properties.visible {
                return Ok(None);
            }
            if current_ref.properties.clip_children {
                let clip = self.world_bounds(current_node)?;
                let Some(intersection) = visible.intersection(clip) else {
                    return Ok(None);
                };
                visible = intersection;
            }
            current = current_ref.parent;
        }
        Ok(Some(visible))
    }

    pub fn ancestor_clip_bounds(&self, node: NodeId) -> Result<Option<Rect>, SceneError> {
        let node_ref = self.require_node(node)?;
        let mut clip: Option<Rect> = None;
        let mut current = node_ref.parent;
        while let Some(current_node) = current {
            let current_ref = self.require_node(current_node)?;
            if !current_ref.properties.visible {
                return Ok(Some(Rect::default()));
            }
            if current_ref.properties.clip_children {
                let bounds = self.world_bounds(current_node)?;
                clip = match clip {
                    Some(clip) => match clip.intersection(bounds) {
                        Some(intersection) => Some(intersection),
                        None => return Ok(Some(Rect::default())),
                    },
                    None => Some(bounds),
                };
            }
            current = current_ref.parent;
        }
        Ok(clip)
    }

    pub fn node_contains_point(&self, node: NodeId, point: Point) -> Result<bool, SceneError> {
        let node_ref = self.require_node(node)?;
        Ok(node_ref.content.contains(self.world_bounds(node)?, point))
    }

    pub fn nodes_overlap(&self, first: NodeId, second: NodeId) -> Result<bool, SceneError> {
        let first_node = self.require_node(first)?;
        let second_node = self.require_node(second)?;
        let first_bounds = self.world_bounds(first)?;
        let second_bounds = self.world_bounds(second)?;
        let Some(intersection) = first_bounds.intersection(second_bounds) else {
            return Ok(false);
        };

        let first_pixel_count = first_node
            .content
            .alpha_mask_pixel_count_in(first_bounds, intersection);
        let second_pixel_count = second_node
            .content
            .alpha_mask_pixel_count_in(second_bounds, intersection);
        let overlaps = match (first_pixel_count, second_pixel_count) {
            (None, None) => true,
            (Some(_), None) => first_node
                .content
                .opaque_pixels_in(first_bounds, intersection)
                .is_some_and(|mut pixels| {
                    pixels.any(|pixel| second_node.content.intersects_rect(second_bounds, pixel))
                }),
            (None, Some(_)) => second_node
                .content
                .opaque_pixels_in(second_bounds, intersection)
                .is_some_and(|mut pixels| {
                    pixels.any(|pixel| first_node.content.intersects_rect(first_bounds, pixel))
                }),
            (Some(first_count), Some(second_count)) if first_count <= second_count => first_node
                .content
                .opaque_pixels_in(first_bounds, intersection)
                .is_some_and(|mut pixels| {
                    pixels.any(|pixel| second_node.content.intersects_rect(second_bounds, pixel))
                }),
            (Some(_), Some(_)) => second_node
                .content
                .opaque_pixels_in(second_bounds, intersection)
                .is_some_and(|mut pixels| {
                    pixels.any(|pixel| first_node.content.intersects_rect(first_bounds, pixel))
                }),
        };
        Ok(overlaps)
    }

    pub fn hit_test(&self, point: Point) -> Result<Option<NodeId>, SceneError> {
        for node in self.paint_order().into_iter().rev() {
            if self.require_node(node)?.properties.input_passthrough {
                continue;
            }
            if self
                .visible_bounds(node)?
                .is_some_and(|bounds| bounds.contains(point))
                && self.node_contains_point(node, point)?
            {
                return Ok(Some(node));
            }
        }
        Ok(None)
    }

    pub fn hit_test_excluding_subtree(
        &self,
        point: Point,
        excluded: NodeId,
    ) -> Result<Option<NodeId>, SceneError> {
        self.require_node(excluded)?;
        for node in self.paint_order().into_iter().rev() {
            if self.is_descendant_of(node, excluded) {
                continue;
            }
            if self.require_node(node)?.properties.input_passthrough {
                continue;
            }
            if self
                .visible_bounds(node)?
                .is_some_and(|bounds| bounds.contains(point))
                && self.node_contains_point(node, point)?
            {
                return Ok(Some(node));
            }
        }
        Ok(None)
    }

    pub fn hit_test_rect(&self, bounds: Rect) -> Result<Vec<NodeId>, SceneError> {
        let mut nodes = Vec::new();
        for node in self.paint_order().into_iter().rev() {
            if self
                .visible_bounds(node)?
                .is_some_and(|visible| visible.intersects(bounds))
            {
                nodes.push(node);
            }
        }
        Ok(nodes)
    }

    pub fn reparent(&mut self, node: NodeId, parent: Option<NodeId>) -> Result<(), SceneError> {
        self.validate_reparent(node, parent)?;
        let current_node = self.require_node(node)?;
        let current_parent = current_node.parent;
        if current_parent == parent {
            return Ok(());
        }
        let layer = parent
            .and_then(|parent| self.node(parent).map(Node::layer))
            .unwrap_or(current_node.layer);

        self.detach(node, current_parent);
        self.require_node_mut(node)?.parent = parent;
        match parent {
            Some(parent) => self.require_node_mut(parent)?.children.push(node),
            None => self.roots.push(node),
        }
        self.set_subtree_layer(node, layer)?;
        Ok(())
    }

    pub fn validate_reparent(
        &self,
        node: NodeId,
        parent: Option<NodeId>,
    ) -> Result<(), SceneError> {
        let current_node = self.require_node(node)?;
        if current_node.parent == parent {
            return Ok(());
        }
        if let Some(parent) = parent {
            self.require_node(parent)?;
            if self.is_descendant_of(parent, node) {
                return Err(SceneError::WouldCreateCycle { node, parent });
            }
        }
        Ok(())
    }

    pub fn remove_subtree(&mut self, node: NodeId) -> Result<(), SceneError> {
        let parent = self.require_node(node)?.parent;
        let mut descendants = Vec::new();
        let mut pending = vec![node];
        while let Some(current) = pending.pop() {
            let current_node = self.require_node(current)?;
            pending.extend(current_node.children.iter().copied());
            descendants.push(current);
        }

        self.detach(node, parent);
        self.node_count -= descendants.len();
        for current in descendants {
            let slot = &mut self.slots[current.index];
            slot.node = None;
            if slot.generation < u64::MAX {
                slot.generation += 1;
                self.free_slots.push(current.index);
            }
        }
        Ok(())
    }

    pub fn paint_order(&self) -> Vec<NodeId> {
        let mut nodes = Vec::with_capacity(self.len());
        for layer in SceneLayer::PAINT_ORDER {
            nodes.extend(self.paint_order_in_layer(layer));
        }
        nodes
    }

    pub fn paint_order_in_layer(&self, layer: SceneLayer) -> Vec<NodeId> {
        let mut nodes = Vec::new();
        let roots = self
            .roots
            .iter()
            .copied()
            .filter(|root| self.node(*root).is_some_and(|node| node.layer == layer))
            .collect::<Vec<_>>();
        let mut pending = self.ordered_siblings(&roots);
        pending.reverse();
        while let Some(node_id) = pending.pop() {
            let Some(node) = self.node(node_id) else {
                continue;
            };
            let visible = node.properties.visible;
            let children = node.children.clone();
            if !visible {
                continue;
            }
            nodes.push(node_id);
            pending.extend(self.ordered_siblings(&children).into_iter().rev());
        }
        nodes
    }

    fn insert_node(
        &mut self,
        parent: Option<NodeId>,
        properties: NodeProperties,
        layer: SceneLayer,
    ) -> NodeId {
        let node = Node {
            parent,
            children: Vec::new(),
            layer,
            properties,
            content: NodeContent::Empty,
        };
        if let Some(index) = self.free_slots.pop() {
            let slot = &mut self.slots[index];
            slot.node = Some(node);
            NodeId {
                index,
                generation: slot.generation,
            }
        } else {
            let index = self.slots.len();
            self.slots.push(Slot {
                generation: 0,
                node: Some(node),
            });
            NodeId {
                index,
                generation: 0,
            }
        }
    }

    fn require_node(&self, node: NodeId) -> Result<&Node, SceneError> {
        self.node(node).ok_or(SceneError::InvalidNode(node))
    }

    fn require_node_mut(&mut self, node: NodeId) -> Result<&mut Node, SceneError> {
        let slot = self
            .slots
            .get_mut(node.index)
            .ok_or(SceneError::InvalidNode(node))?;
        if slot.generation != node.generation {
            return Err(SceneError::InvalidNode(node));
        }
        slot.node.as_mut().ok_or(SceneError::InvalidNode(node))
    }

    fn is_descendant_of(&self, node: NodeId, ancestor: NodeId) -> bool {
        let mut current = Some(node);
        while let Some(node) = current {
            if node == ancestor {
                return true;
            }
            current = self.node(node).and_then(Node::parent);
        }
        false
    }

    fn set_subtree_layer(&mut self, node: NodeId, layer: SceneLayer) -> Result<(), SceneError> {
        let mut pending = vec![node];
        while let Some(current) = pending.pop() {
            let children = self.require_node(current)?.children.clone();
            self.require_node_mut(current)?.layer = layer;
            pending.extend(children);
        }
        Ok(())
    }

    fn detach(&mut self, node: NodeId, parent: Option<NodeId>) {
        match parent {
            Some(parent) => {
                if let Some(parent) = self.node_mut_if_valid(parent) {
                    parent.children.retain(|child| *child != node);
                }
            }
            None => self.roots.retain(|root| *root != node),
        }
    }

    fn node_mut_if_valid(&mut self, node: NodeId) -> Option<&mut Node> {
        let slot = self.slots.get_mut(node.index)?;
        (slot.generation == node.generation)
            .then_some(slot.node.as_mut())
            .flatten()
    }

    fn ordered_siblings(&self, siblings: &[NodeId]) -> Vec<NodeId> {
        let mut ordered = siblings.to_vec();
        ordered.sort_by_key(|node| self.node(*node).map(|node| node.properties.z_index));
        ordered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn properties(z_index: i32) -> NodeProperties {
        NodeProperties {
            z_index,
            ..NodeProperties::default()
        }
    }

    #[test]
    fn rejects_invalid_geometry() {
        assert_eq!(
            Transform::new(f64::NAN, 0.0),
            Err(GeometryError::NonFiniteCoordinate)
        );
        assert_eq!(
            Rect::new(0.0, 0.0, -1.0, 1.0),
            Err(GeometryError::NegativeExtent)
        );
        assert_eq!(
            Point::new(f64::INFINITY, 0.0),
            Err(GeometryError::NonFiniteCoordinate)
        );
        assert_eq!(
            Rect::new(f64::MAX, 0.0, 1.0, 1.0),
            Err(GeometryError::ExtentOverflow)
        );
    }

    #[test]
    fn uses_half_open_rectangles_for_containment_and_intersection() {
        let bounds = Rect::new(10.0, 20.0, 30.0, 40.0).unwrap();

        assert!(bounds.contains(Point::new(10.0, 20.0).unwrap()));
        assert!(bounds.contains(Point::new(39.999, 59.999).unwrap()));
        assert!(!bounds.contains(Point::new(40.0, 20.0).unwrap()));
        assert!(!bounds.contains(Point::new(10.0, 60.0).unwrap()));
        assert!(!bounds.intersects(Rect::new(40.0, 20.0, 1.0, 1.0).unwrap()));
        assert!(!bounds.intersects(Rect::new(10.0, 60.0, 1.0, 1.0).unwrap()));
        assert!(!bounds.intersects(Rect::new(10.0, 20.0, 0.0, 1.0).unwrap()));
    }

    #[test]
    fn invalidates_a_removed_handle_before_reusing_its_slot() {
        let mut scene = Scene::new();
        let first = scene.create_root(NodeProperties::default());
        scene.remove_subtree(first).unwrap();
        let replacement = scene.create_root(NodeProperties::default());

        assert_ne!(first, replacement);
        assert!(scene.node(first).is_none());
        assert!(scene.node(replacement).is_some());
    }

    #[test]
    fn rejects_a_removed_handle_as_a_parent() {
        let mut scene = Scene::new();
        let removed = scene.create_root(NodeProperties::default());
        let root = scene.create_root(NodeProperties::default());
        scene.remove_subtree(removed).unwrap();

        assert_eq!(
            scene.create_child(removed, NodeProperties::default()),
            Err(SceneError::InvalidNode(removed))
        );
        assert_eq!(scene.node(root).unwrap().children(), &[]);
    }

    #[test]
    fn updates_node_properties_without_exposing_structural_mutation() {
        let mut scene = Scene::new();
        let node = scene.create_root(NodeProperties::default());
        let properties = NodeProperties {
            transform: Transform::new(12.0, 34.0).unwrap(),
            bounds: Rect::new(1.0, 2.0, 320.0, 180.0).unwrap(),
            visible: false,
            clip_children: false,
            z_index: 7,
            overlap_enabled: false,
            external_drop_enabled: false,
            input_passthrough: false,
        };

        scene.set_properties(node, properties).unwrap();

        assert_eq!(scene.node(node).unwrap().properties(), properties);
        assert!(scene.paint_order().is_empty());
    }

    #[test]
    fn attaches_an_image_without_coupling_the_scene_to_the_texture_store() {
        let mut scene = Scene::new();
        let node = scene.create_root(NodeProperties::default());
        let mut textures = crate::resources::TextureStore::new();
        let texture =
            textures.insert(crate::resources::Texture::from_rgba8(1, 1, vec![0, 0, 0, 0]).unwrap());
        let image = ImageNode::new(texture, 0.5).unwrap();

        scene
            .set_content(node, NodeContent::Image(image.clone()))
            .unwrap();

        assert_eq!(
            scene.node(node).unwrap().content(),
            &NodeContent::Image(image)
        );
        assert_eq!(
            ImageNode::new(texture, 1.1),
            Err(ImageNodeError::InvalidOpacity)
        );
    }

    #[test]
    fn computes_bounds_from_all_ancestor_transforms() {
        let mut scene = Scene::new();
        let root = scene.create_root(NodeProperties {
            transform: Transform::new(40.0, 20.0).unwrap(),
            ..NodeProperties::default()
        });
        let child = scene
            .create_child(
                root,
                NodeProperties {
                    transform: Transform::new(8.0, 6.0).unwrap(),
                    bounds: Rect::new(1.0, 2.0, 128.0, 64.0).unwrap(),
                    ..NodeProperties::default()
                },
            )
            .unwrap();

        assert_eq!(
            scene.world_bounds(child).unwrap(),
            Rect::new(49.0, 28.0, 128.0, 64.0).unwrap()
        );
    }

    #[test]
    fn hits_the_topmost_visible_node_at_a_point() {
        let mut scene = Scene::new();
        let bounds = Rect::new(0.0, 0.0, 100.0, 100.0).unwrap();
        let back = scene.create_root(NodeProperties {
            bounds,
            ..properties(0)
        });
        let same_depth = scene.create_root(NodeProperties {
            bounds,
            ..properties(0)
        });
        let front = scene.create_root(NodeProperties {
            bounds,
            ..properties(1)
        });
        let hidden = scene.create_root(NodeProperties {
            bounds,
            visible: false,
            ..properties(2)
        });

        assert_eq!(
            scene.hit_test(Point::new(50.0, 50.0).unwrap()).unwrap(),
            Some(front)
        );
        assert_ne!(back, same_depth);
        assert_ne!(front, hidden);
        assert_eq!(
            scene.hit_test(Point::new(100.0, 100.0).unwrap()).unwrap(),
            None
        );
    }

    #[test]
    fn image_alpha_contact_and_passthrough_share_pointer_targeting() {
        let mut scene = Scene::new();
        let bounds = Rect::new(0.0, 0.0, 4.0, 4.0).unwrap();
        let back = scene.create_root(NodeProperties {
            bounds,
            ..NodeProperties::default()
        });
        let mut textures = crate::resources::TextureStore::new();
        let texture = textures.insert(
            crate::resources::Texture::from_rgba8(
                2,
                2,
                vec![0, 0, 0, 0, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 0],
            )
            .unwrap(),
        );
        let mask = ImageAlphaMask::new(2, 2, textures.texture(texture).unwrap().alpha()).unwrap();
        let front = scene.create_root(NodeProperties {
            bounds,
            z_index: 1,
            ..NodeProperties::default()
        });
        scene
            .set_content(
                front,
                NodeContent::Image(
                    ImageNode::with_contact_shape(
                        texture,
                        1.0,
                        ImageContactShape::alpha(),
                        Some(mask),
                    )
                    .unwrap(),
                ),
            )
            .unwrap();

        assert_eq!(
            scene.hit_test(Point::new(0.5, 0.5).unwrap()).unwrap(),
            Some(back)
        );
        assert_eq!(
            scene.hit_test(Point::new(2.5, 0.5).unwrap()).unwrap(),
            Some(front)
        );

        let mut properties = scene.node(front).unwrap().properties();
        properties.input_passthrough = true;
        scene.set_properties(front, properties).unwrap();
        assert_eq!(
            scene.hit_test(Point::new(2.5, 0.5).unwrap()).unwrap(),
            Some(back)
        );

        let child = scene
            .create_child(
                front,
                NodeProperties {
                    bounds: Rect::new(0.0, 0.0, 2.0, 2.0).unwrap(),
                    ..NodeProperties::default()
                },
            )
            .unwrap();
        assert_eq!(
            scene.hit_test(Point::new(0.5, 0.5).unwrap()).unwrap(),
            Some(child)
        );
    }

    #[test]
    fn image_alpha_contact_controls_overlap_without_changing_bounds() {
        let mut scene = Scene::new();
        let bounds = Rect::new(0.0, 0.0, 2.0, 2.0).unwrap();
        let mut textures = crate::resources::TextureStore::new();
        let first_texture = textures.insert(
            crate::resources::Texture::from_rgba8(
                2,
                2,
                vec![0, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            )
            .unwrap(),
        );
        let second_texture = textures.insert(
            crate::resources::Texture::from_rgba8(
                2,
                2,
                vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255],
            )
            .unwrap(),
        );
        let first = scene.create_root(NodeProperties {
            bounds,
            ..NodeProperties::default()
        });
        let second = scene.create_root(NodeProperties {
            bounds,
            ..NodeProperties::default()
        });
        let first_mask =
            ImageAlphaMask::new(2, 2, textures.texture(first_texture).unwrap().alpha()).unwrap();
        let second_mask =
            ImageAlphaMask::new(2, 2, textures.texture(second_texture).unwrap().alpha()).unwrap();
        scene
            .set_content(
                first,
                NodeContent::Image(
                    ImageNode::with_contact_shape(
                        first_texture,
                        1.0,
                        ImageContactShape::alpha(),
                        Some(first_mask),
                    )
                    .unwrap(),
                ),
            )
            .unwrap();
        scene
            .set_content(
                second,
                NodeContent::Image(
                    ImageNode::with_contact_shape(
                        second_texture,
                        1.0,
                        ImageContactShape::alpha(),
                        Some(second_mask),
                    )
                    .unwrap(),
                ),
            )
            .unwrap();

        assert!(
            scene
                .world_bounds(first)
                .unwrap()
                .intersects(scene.world_bounds(second).unwrap())
        );
        assert!(!scene.nodes_overlap(first, second).unwrap());
    }

    #[test]
    fn hit_testing_preserves_hierarchical_stacking_contexts() {
        let mut scene = Scene::new();
        let bounds = Rect::new(0.0, 0.0, 100.0, 100.0).unwrap();
        let back = scene.create_root(NodeProperties {
            bounds,
            ..properties(0)
        });
        scene
            .create_child(
                back,
                NodeProperties {
                    bounds,
                    ..properties(100)
                },
            )
            .unwrap();
        let front = scene.create_root(NodeProperties {
            bounds,
            ..properties(1)
        });

        assert_eq!(
            scene.hit_test(Point::new(50.0, 50.0).unwrap()).unwrap(),
            Some(front)
        );
    }

    #[test]
    fn allows_children_outside_a_parent_without_clipping() {
        let mut scene = Scene::new();
        let parent = scene.create_root(NodeProperties {
            bounds: Rect::new(0.0, 0.0, 10.0, 10.0).unwrap(),
            ..NodeProperties::default()
        });
        let child = scene
            .create_child(
                parent,
                NodeProperties {
                    transform: Transform::new(8.0, 0.0).unwrap(),
                    bounds: Rect::new(0.0, 0.0, 10.0, 10.0).unwrap(),
                    ..NodeProperties::default()
                },
            )
            .unwrap();

        assert_eq!(
            scene.hit_test(Point::new(15.0, 5.0).unwrap()).unwrap(),
            Some(child)
        );
        assert_eq!(
            scene.visible_bounds(child).unwrap(),
            Some(Rect::new(8.0, 0.0, 10.0, 10.0).unwrap())
        );
        assert_eq!(scene.ancestor_clip_bounds(child).unwrap(), None);
    }

    #[test]
    fn clips_descendants_for_render_bounds_and_hit_testing() {
        let mut scene = Scene::new();
        let parent = scene.create_root(NodeProperties {
            bounds: Rect::new(0.0, 0.0, 10.0, 10.0).unwrap(),
            clip_children: true,
            ..NodeProperties::default()
        });
        let child = scene
            .create_child(
                parent,
                NodeProperties {
                    transform: Transform::new(8.0, 0.0).unwrap(),
                    bounds: Rect::new(0.0, 0.0, 10.0, 10.0).unwrap(),
                    ..NodeProperties::default()
                },
            )
            .unwrap();

        assert_eq!(
            scene.visible_bounds(child).unwrap(),
            Some(Rect::new(8.0, 0.0, 2.0, 10.0).unwrap())
        );
        assert_eq!(
            scene.ancestor_clip_bounds(child).unwrap(),
            Some(Rect::new(0.0, 0.0, 10.0, 10.0).unwrap())
        );
        assert_eq!(
            scene.hit_test(Point::new(9.0, 5.0).unwrap()).unwrap(),
            Some(child)
        );
        assert_eq!(
            scene.hit_test(Point::new(15.0, 5.0).unwrap()).unwrap(),
            None
        );
        assert_eq!(
            scene
                .hit_test_rect(Rect::new(10.0, 0.0, 8.0, 10.0).unwrap())
                .unwrap(),
            Vec::<NodeId>::new()
        );
    }

    #[test]
    fn layers_paint_and_hit_test_underlay_before_overlay() {
        let mut scene = Scene::new();
        let bounds = Rect::new(0.0, 0.0, 10.0, 10.0).unwrap();
        let overlay = scene.create_root_in_layer(
            NodeProperties {
                bounds,
                ..properties(-10)
            },
            SceneLayer::Overlay,
        );
        let underlay = scene.create_root_in_layer(
            NodeProperties {
                bounds,
                ..properties(10)
            },
            SceneLayer::Underlay,
        );

        assert_eq!(scene.paint_order(), vec![underlay, overlay]);
        assert_eq!(
            scene.paint_order_in_layer(SceneLayer::Underlay),
            vec![underlay]
        );
        assert_eq!(
            scene.paint_order_in_layer(SceneLayer::Overlay),
            vec![overlay]
        );
        assert_eq!(
            scene.hit_test(Point::new(5.0, 5.0).unwrap()).unwrap(),
            Some(overlay)
        );
    }

    #[test]
    fn roots_propagate_their_layer_to_descendants_and_reparented_subtrees() {
        let mut scene = Scene::new();
        let underlay = scene.create_root_in_layer(NodeProperties::default(), SceneLayer::Underlay);
        let child = scene
            .create_child(underlay, NodeProperties::default())
            .unwrap();
        let overlay = scene.create_root(NodeProperties::default());

        assert_eq!(scene.node(underlay).unwrap().layer(), SceneLayer::Underlay);
        assert_eq!(scene.node(child).unwrap().layer(), SceneLayer::Underlay);
        scene.reparent(underlay, Some(overlay)).unwrap();
        assert_eq!(scene.node(underlay).unwrap().layer(), SceneLayer::Overlay);
        assert_eq!(scene.node(child).unwrap().layer(), SceneLayer::Overlay);
        assert_eq!(
            scene.set_root_layer(child, SceneLayer::Underlay),
            Err(SceneError::LayerRequiresRoot(child))
        );
        scene.set_root_layer(overlay, SceneLayer::Underlay).unwrap();
        assert_eq!(scene.node(overlay).unwrap().layer(), SceneLayer::Underlay);
        assert_eq!(scene.node(underlay).unwrap().layer(), SceneLayer::Underlay);
        assert_eq!(scene.node(child).unwrap().layer(), SceneLayer::Underlay);
    }

    #[test]
    fn hit_testing_can_exclude_a_node_and_its_descendants() {
        let mut scene = Scene::new();
        let bounds = Rect::new(0.0, 0.0, 100.0, 100.0).unwrap();
        let parent = scene.create_root(NodeProperties {
            bounds,
            ..properties(0)
        });
        let source = scene
            .create_child(
                parent,
                NodeProperties {
                    bounds,
                    ..properties(1)
                },
            )
            .unwrap();
        let child = scene
            .create_child(
                source,
                NodeProperties {
                    bounds,
                    ..properties(2)
                },
            )
            .unwrap();

        assert_eq!(
            scene
                .hit_test_excluding_subtree(Point::new(50.0, 50.0).unwrap(), source)
                .unwrap(),
            Some(parent)
        );
        assert!(
            scene
                .hit_test_excluding_subtree(Point::new(50.0, 50.0).unwrap(), child)
                .is_ok()
        );
        scene.remove_subtree(source).unwrap();
        assert!(
            scene
                .hit_test_excluding_subtree(Point::new(50.0, 50.0).unwrap(), source)
                .is_err()
        );
    }

    #[test]
    fn returns_overlapping_nodes_front_to_back_for_rect_queries() {
        let mut scene = Scene::new();
        let bounds = Rect::new(0.0, 0.0, 20.0, 20.0).unwrap();
        let back = scene.create_root(NodeProperties {
            bounds,
            ..properties(0)
        });
        let middle = scene.create_root(NodeProperties {
            bounds,
            ..properties(0)
        });
        let front = scene.create_root(NodeProperties {
            bounds,
            ..properties(1)
        });

        assert_eq!(
            scene
                .hit_test_rect(Rect::new(10.0, 10.0, 5.0, 5.0).unwrap())
                .unwrap(),
            vec![front, middle, back]
        );
    }

    #[test]
    fn removes_a_subtree_and_preserves_other_roots() {
        let mut scene = Scene::new();
        let removed_root = scene.create_root(NodeProperties::default());
        let child = scene
            .create_child(removed_root, NodeProperties::default())
            .unwrap();
        let grandchild = scene
            .create_child(child, NodeProperties::default())
            .unwrap();
        let surviving_root = scene.create_root(NodeProperties::default());

        scene.remove_subtree(removed_root).unwrap();

        assert_eq!(scene.roots(), &[surviving_root]);
        assert!(scene.node(removed_root).is_none());
        assert!(scene.node(child).is_none());
        assert!(scene.node(grandchild).is_none());
        assert_eq!(scene.len(), 1);
    }

    #[test]
    fn rejects_cycles_when_reparenting() {
        let mut scene = Scene::new();
        let root = scene.create_root(NodeProperties::default());
        let child = scene.create_child(root, NodeProperties::default()).unwrap();

        assert_eq!(
            scene.reparent(root, Some(child)),
            Err(SceneError::WouldCreateCycle {
                node: root,
                parent: child,
            })
        );
    }

    #[test]
    fn reparenting_appends_to_the_new_sibling_order() {
        let mut scene = Scene::new();
        let source = scene.create_root(NodeProperties::default());
        let destination = scene.create_root(NodeProperties::default());
        let first = scene.create_child(destination, properties(0)).unwrap();
        let moved = scene.create_child(source, properties(0)).unwrap();

        scene.reparent(moved, Some(destination)).unwrap();

        assert_eq!(scene.node(destination).unwrap().children(), &[first, moved]);
        assert_eq!(scene.paint_order(), vec![source, destination, first, moved]);
    }

    #[test]
    fn paints_complete_subtrees_in_sibling_stacking_order() {
        let mut scene = Scene::new();
        let back = scene.create_root(properties(0));
        let back_high_child = scene.create_child(back, properties(100)).unwrap();
        let back_low_child = scene.create_child(back, properties(-100)).unwrap();
        let front = scene.create_root(properties(1));
        let front_child = scene.create_child(front, properties(0)).unwrap();

        assert_eq!(
            scene.paint_order(),
            vec![back, back_low_child, back_high_child, front, front_child]
        );
    }

    #[test]
    fn hides_entire_invisible_subtrees_from_paint_order() {
        let mut scene = Scene::new();
        let root = scene.create_root(NodeProperties {
            visible: false,
            ..NodeProperties::default()
        });
        let child = scene.create_child(root, NodeProperties::default()).unwrap();

        assert!(scene.paint_order().is_empty());
        assert!(scene.node(child).is_some());
    }

    #[test]
    fn traverses_deep_trees_without_recursing() {
        let mut scene = Scene::new();
        let mut node = scene.create_root(NodeProperties::default());
        for _ in 0..4_096 {
            node = scene.create_child(node, NodeProperties::default()).unwrap();
        }

        assert_eq!(scene.paint_order().len(), 4_097);
    }
}

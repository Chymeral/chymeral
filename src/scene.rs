use std::{
    error::Error,
    fmt::{Display, Formatter},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId {
    index: usize,
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryError {
    NonFiniteCoordinate,
    NegativeExtent,
}

impl Display for GeometryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonFiniteCoordinate => formatter.write_str("geometry coordinates must be finite"),
            Self::NegativeExtent => formatter.write_str("rectangle extents cannot be negative"),
        }
    }
}

impl Error for GeometryError {}

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
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeProperties {
    pub transform: Transform,
    pub bounds: Rect,
    pub visible: bool,
    pub z_index: i32,
}

impl Default for NodeProperties {
    fn default() -> Self {
        Self {
            transform: Transform::default(),
            bounds: Rect::default(),
            visible: true,
            z_index: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    properties: NodeProperties,
}

impl Node {
    pub const fn parent(&self) -> Option<NodeId> {
        self.parent
    }

    pub fn children(&self) -> &[NodeId] {
        &self.children
    }

    pub const fn properties(&self) -> NodeProperties {
        self.properties
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SceneError {
    InvalidNode(NodeId),
    WouldCreateCycle { node: NodeId, parent: NodeId },
}

impl Display for SceneError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidNode(_) => formatter.write_str("node handle is no longer valid"),
            Self::WouldCreateCycle { .. } => {
                formatter.write_str("a scene node cannot parent one of its descendants")
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
        let node = self.insert_node(None, properties);
        self.roots.push(node);
        self.node_count += 1;
        node
    }

    pub fn create_child(
        &mut self,
        parent: NodeId,
        properties: NodeProperties,
    ) -> Result<NodeId, SceneError> {
        self.require_node(parent)?;
        let child = self.insert_node(Some(parent), properties);
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

    pub fn reparent(&mut self, node: NodeId, parent: Option<NodeId>) -> Result<(), SceneError> {
        let current_parent = self.require_node(node)?.parent;
        if current_parent == parent {
            return Ok(());
        }
        if let Some(parent) = parent {
            self.require_node(parent)?;
            if self.is_descendant_of(parent, node) {
                return Err(SceneError::WouldCreateCycle { node, parent });
            }
        }

        self.detach(node, current_parent);
        self.require_node_mut(node)?.parent = parent;
        match parent {
            Some(parent) => self.require_node_mut(parent)?.children.push(node),
            None => self.roots.push(node),
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
        let mut pending = self.ordered_siblings(&self.roots);
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

    fn insert_node(&mut self, parent: Option<NodeId>, properties: NodeProperties) -> NodeId {
        let node = Node {
            parent,
            children: Vec::new(),
            properties,
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
            z_index: 7,
        };

        scene.set_properties(node, properties).unwrap();

        assert_eq!(scene.node(node).unwrap().properties(), properties);
        assert!(scene.paint_order().is_empty());
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

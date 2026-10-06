use std::{
    error::Error,
    fmt::{Display, Formatter},
};

use crate::{
    runtime::{Modifiers, PointerButton, PointerEvent, SceneEvent, Vec2},
    scene::{NodeId, Point, Scene, SceneError},
};

pub const DRAG_START_DISTANCE: f64 = 4.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InteractionError {
    InvalidPointerPosition,
    Scene(SceneError),
}

impl Display for InteractionError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPointerPosition => {
                formatter.write_str("pointer position must use finite coordinates")
            }
            Self::Scene(error) => error.fmt(formatter),
        }
    }
}

impl Error for InteractionError {}

impl From<SceneError> for InteractionError {
    fn from(error: SceneError) -> Self {
        Self::Scene(error)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum DragState {
    #[default]
    None,
    Pending {
        source: NodeId,
        origin: Vec2,
    },
    Active {
        source: NodeId,
        origin: Vec2,
        position: Vec2,
        target: Option<NodeId>,
    },
}

#[derive(Debug, Default)]
pub struct SceneInteraction {
    hovered_node: Option<NodeId>,
    focused_node: Option<NodeId>,
    pointer_position: Option<Vec2>,
    pointer_modifiers: Modifiers,
    drag_state: DragState,
}

impl SceneInteraction {
    pub fn new() -> Self {
        Self::default()
    }

    pub const fn hovered_node(&self) -> Option<NodeId> {
        self.hovered_node
    }

    pub const fn focused_node(&self) -> Option<NodeId> {
        self.focused_node
    }

    pub const fn drag_source(&self) -> Option<NodeId> {
        match self.drag_state {
            DragState::Active { source, .. } => Some(source),
            DragState::None | DragState::Pending { .. } => None,
        }
    }

    pub fn route_pointer(
        &mut self,
        scene: &Scene,
        event: &PointerEvent,
    ) -> Result<Vec<SceneEvent>, InteractionError> {
        let (position, modifiers) = pointer_details(event);
        let target = self.target_at(scene, position)?;
        self.pointer_position = Some(position);
        self.pointer_modifiers = modifiers;

        let mut events = Vec::new();
        self.reconcile_focus(scene, &mut events);
        self.update_hover(target, position, modifiers, &mut events);
        match event {
            PointerEvent::Move { .. } => {
                if let Some(target) = target {
                    events.push(SceneEvent::PointerMove {
                        target,
                        position,
                        modifiers,
                    });
                }
                self.advance_drag(scene, position, modifiers, &mut events)?;
            }
            PointerEvent::Press { button, .. } => {
                if *button == PointerButton::Primary {
                    self.cancel_drag(&mut events);
                    self.update_focus(target, &mut events);
                }
                if let Some(target) = target {
                    events.push(SceneEvent::PointerPress {
                        target,
                        button: *button,
                        position,
                        modifiers,
                    });
                }
                if *button == PointerButton::Primary {
                    self.drag_state = target.map_or(DragState::None, |source| DragState::Pending {
                        source,
                        origin: position,
                    });
                }
            }
            PointerEvent::Release { button, .. } => {
                if let Some(target) = target {
                    events.push(SceneEvent::PointerRelease {
                        target,
                        button: *button,
                        position,
                        modifiers,
                    });
                }
                if *button == PointerButton::Primary {
                    self.finish_drag(scene, position, modifiers, &mut events)?;
                }
            }
        }
        Ok(events)
    }

    pub fn reconcile(&mut self, scene: &Scene) -> Result<Vec<SceneEvent>, InteractionError> {
        let mut events = Vec::new();
        if let Some(position) = self.pointer_position {
            let target = self.target_at(scene, position)?;
            self.update_hover(target, position, self.pointer_modifiers, &mut events);
        }
        self.reconcile_focus(scene, &mut events);
        self.reconcile_drag(scene, &mut events)?;
        Ok(events)
    }

    pub fn pointer_left(&mut self) -> Vec<SceneEvent> {
        let mut events = Vec::new();
        if let Some(position) = self.pointer_position {
            self.update_hover(None, position, self.pointer_modifiers, &mut events);
        }
        self.pointer_position = None;
        self.clear_drag_target(&mut events);
        events
    }

    pub fn window_focus_lost(&mut self) -> Vec<SceneEvent> {
        let mut events = Vec::new();
        self.cancel_drag(&mut events);
        events
    }

    fn advance_drag(
        &mut self,
        scene: &Scene,
        position: Vec2,
        modifiers: Modifiers,
        events: &mut Vec<SceneEvent>,
    ) -> Result<(), InteractionError> {
        match self.drag_state {
            DragState::None => Ok(()),
            DragState::Pending { source, origin } => {
                if !node_is_painted(scene, source) {
                    self.drag_state = DragState::None;
                    return Ok(());
                }
                if squared_distance(origin, position) < DRAG_START_DISTANCE.powi(2) {
                    return Ok(());
                }

                self.drag_state = DragState::Active {
                    source,
                    origin,
                    position,
                    target: None,
                };
                events.push(SceneEvent::DragStart {
                    source,
                    origin,
                    position,
                    modifiers,
                });
                let target = self.drag_target_at(scene, source, position)?;
                self.transition_drag_target(source, target, position, modifiers, events);
                events.push(SceneEvent::DragMove {
                    source,
                    target,
                    position,
                    modifiers,
                });
                Ok(())
            }
            DragState::Active { source, .. } => {
                if !node_is_painted(scene, source) {
                    self.cancel_drag(events);
                    return Ok(());
                }

                self.set_drag_position(position);
                let target = self.drag_target_at(scene, source, position)?;
                self.transition_drag_target(source, target, position, modifiers, events);
                events.push(SceneEvent::DragMove {
                    source,
                    target,
                    position,
                    modifiers,
                });
                Ok(())
            }
        }
    }

    fn finish_drag(
        &mut self,
        scene: &Scene,
        position: Vec2,
        modifiers: Modifiers,
        events: &mut Vec<SceneEvent>,
    ) -> Result<(), InteractionError> {
        let DragState::Active { source, .. } = self.drag_state else {
            self.drag_state = DragState::None;
            return Ok(());
        };
        if !node_is_painted(scene, source) {
            self.cancel_drag(events);
            return Ok(());
        }

        self.set_drag_position(position);
        let target = self.drag_target_at(scene, source, position)?;
        self.transition_drag_target(source, target, position, modifiers, events);
        if let Some(target) = target {
            events.push(SceneEvent::Drop {
                source,
                target,
                position,
                modifiers,
            });
        }
        events.push(SceneEvent::DragEnd {
            source,
            target,
            position,
            modifiers,
        });
        self.drag_state = DragState::None;
        Ok(())
    }

    fn reconcile_drag(
        &mut self,
        scene: &Scene,
        events: &mut Vec<SceneEvent>,
    ) -> Result<(), InteractionError> {
        match self.drag_state {
            DragState::None => Ok(()),
            DragState::Pending { source, .. } => {
                if !node_is_painted(scene, source) {
                    self.drag_state = DragState::None;
                }
                Ok(())
            }
            DragState::Active {
                source, position, ..
            } => {
                if !node_is_painted(scene, source) {
                    self.cancel_drag(events);
                    return Ok(());
                }
                let target = match self.pointer_position {
                    Some(position) => self.drag_target_at(scene, source, position)?,
                    None => None,
                };
                self.transition_drag_target(
                    source,
                    target,
                    position,
                    self.pointer_modifiers,
                    events,
                );
                Ok(())
            }
        }
    }

    fn target_at(&self, scene: &Scene, position: Vec2) -> Result<Option<NodeId>, InteractionError> {
        let point = Point::new(position.x, position.y)
            .map_err(|_| InteractionError::InvalidPointerPosition)?;
        scene.hit_test(point).map_err(InteractionError::from)
    }

    fn drag_target_at(
        &self,
        scene: &Scene,
        source: NodeId,
        position: Vec2,
    ) -> Result<Option<NodeId>, InteractionError> {
        let point = Point::new(position.x, position.y)
            .map_err(|_| InteractionError::InvalidPointerPosition)?;
        scene
            .hit_test_excluding_subtree(point, source)
            .map_err(InteractionError::from)
    }

    fn update_hover(
        &mut self,
        target: Option<NodeId>,
        position: Vec2,
        modifiers: Modifiers,
        events: &mut Vec<SceneEvent>,
    ) {
        if self.hovered_node == target {
            return;
        }
        if let Some(previous) = self.hovered_node.take() {
            events.push(SceneEvent::PointerLeave {
                target: previous,
                position,
                modifiers,
            });
        }
        self.hovered_node = target;
        if let Some(target) = target {
            events.push(SceneEvent::PointerEnter {
                target,
                position,
                modifiers,
            });
        }
    }

    fn update_focus(&mut self, target: Option<NodeId>, events: &mut Vec<SceneEvent>) {
        if self.focused_node == target {
            return;
        }
        if let Some(previous) = self.focused_node.take() {
            events.push(SceneEvent::FocusLost { target: previous });
        }
        self.focused_node = target;
        if let Some(target) = target {
            events.push(SceneEvent::FocusGained { target });
        }
    }

    fn reconcile_focus(&mut self, scene: &Scene, events: &mut Vec<SceneEvent>) {
        if self
            .focused_node
            .is_some_and(|node| scene.node(node).is_none())
        {
            self.update_focus(None, events);
        }
    }

    fn set_drag_position(&mut self, position: Vec2) {
        if let DragState::Active {
            position: stored_position,
            ..
        } = &mut self.drag_state
        {
            *stored_position = position;
        }
    }

    fn transition_drag_target(
        &mut self,
        source: NodeId,
        target: Option<NodeId>,
        position: Vec2,
        modifiers: Modifiers,
        events: &mut Vec<SceneEvent>,
    ) {
        let DragState::Active {
            target: previous, ..
        } = self.drag_state
        else {
            return;
        };
        if previous == target {
            return;
        }
        if let Some(previous) = previous {
            events.push(SceneEvent::DragLeave {
                source,
                target: previous,
                position,
                modifiers,
            });
        }
        if let DragState::Active {
            target: stored_target,
            ..
        } = &mut self.drag_state
        {
            *stored_target = target;
        }
        if let Some(target) = target {
            events.push(SceneEvent::DragEnter {
                source,
                target,
                position,
                modifiers,
            });
        }
    }

    fn clear_drag_target(&mut self, events: &mut Vec<SceneEvent>) {
        let DragState::Active {
            source, position, ..
        } = self.drag_state
        else {
            return;
        };
        self.transition_drag_target(source, None, position, self.pointer_modifiers, events);
    }

    fn cancel_drag(&mut self, events: &mut Vec<SceneEvent>) {
        let DragState::Active {
            source,
            position,
            target,
            ..
        } = self.drag_state
        else {
            self.drag_state = DragState::None;
            return;
        };
        if let Some(target) = target {
            events.push(SceneEvent::DragLeave {
                source,
                target,
                position,
                modifiers: self.pointer_modifiers,
            });
        }
        events.push(SceneEvent::DragCancel {
            source,
            position,
            modifiers: self.pointer_modifiers,
        });
        self.drag_state = DragState::None;
    }
}

fn pointer_details(event: &PointerEvent) -> (Vec2, Modifiers) {
    match event {
        PointerEvent::Move {
            position,
            modifiers,
        }
        | PointerEvent::Press {
            position,
            modifiers,
            ..
        }
        | PointerEvent::Release {
            position,
            modifiers,
            ..
        } => (*position, *modifiers),
    }
}

fn node_is_painted(scene: &Scene, node: NodeId) -> bool {
    scene.paint_order().contains(&node)
}

fn squared_distance(first: Vec2, second: Vec2) -> f64 {
    let x = second.x - first.x;
    let y = second.y - first.y;
    x.mul_add(x, y * y)
}

#[cfg(test)]
mod tests {
    use crate::{
        runtime::{Modifiers, PointerButton, PointerEvent, SceneEvent, Vec2},
        scene::{NodeId, NodeProperties, Rect, Scene},
    };

    use super::{DRAG_START_DISTANCE, InteractionError, SceneInteraction};

    fn scene_with_node() -> (Scene, NodeId) {
        let mut scene = Scene::new();
        let node = scene.create_root(NodeProperties {
            bounds: Rect::new(0.0, 0.0, 10.0, 10.0).unwrap(),
            ..NodeProperties::default()
        });
        (scene, node)
    }

    fn scene_with_drag_target() -> (Scene, NodeId, NodeId) {
        let mut scene = Scene::new();
        let source = scene.create_root(NodeProperties {
            bounds: Rect::new(0.0, 0.0, 10.0, 10.0).unwrap(),
            ..NodeProperties::default()
        });
        let target = scene.create_root(NodeProperties {
            bounds: Rect::new(20.0, 0.0, 10.0, 10.0).unwrap(),
            ..NodeProperties::default()
        });
        (scene, source, target)
    }

    fn pointer_move(x: f64, y: f64) -> PointerEvent {
        PointerEvent::Move {
            position: Vec2 { x, y },
            modifiers: Modifiers::default(),
        }
    }

    fn primary_press(x: f64, y: f64) -> PointerEvent {
        PointerEvent::Press {
            button: PointerButton::Primary,
            position: Vec2 { x, y },
            modifiers: Modifiers::default(),
        }
    }

    fn primary_release(x: f64, y: f64) -> PointerEvent {
        PointerEvent::Release {
            button: PointerButton::Primary,
            position: Vec2 { x, y },
            modifiers: Modifiers::default(),
        }
    }

    #[test]
    fn emits_hover_transitions_and_targeted_pointer_moves() {
        let (scene, node) = scene_with_node();
        let mut interaction = SceneInteraction::new();

        assert_eq!(
            interaction
                .route_pointer(&scene, &pointer_move(5.0, 5.0))
                .unwrap(),
            vec![
                SceneEvent::PointerEnter {
                    target: node,
                    position: Vec2 { x: 5.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
                SceneEvent::PointerMove {
                    target: node,
                    position: Vec2 { x: 5.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
            ]
        );
        assert_eq!(interaction.hovered_node(), Some(node));
        assert_eq!(
            interaction
                .route_pointer(&scene, &pointer_move(12.0, 5.0))
                .unwrap(),
            vec![SceneEvent::PointerLeave {
                target: node,
                position: Vec2 { x: 12.0, y: 5.0 },
                modifiers: Modifiers::default(),
            }]
        );
        assert_eq!(interaction.hovered_node(), None);
    }

    #[test]
    fn primary_press_changes_focus_without_pointer_capture() {
        let (scene, node) = scene_with_node();
        let mut interaction = SceneInteraction::new();

        assert_eq!(
            interaction
                .route_pointer(&scene, &primary_press(5.0, 5.0))
                .unwrap(),
            vec![
                SceneEvent::PointerEnter {
                    target: node,
                    position: Vec2 { x: 5.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
                SceneEvent::FocusGained { target: node },
                SceneEvent::PointerPress {
                    target: node,
                    button: PointerButton::Primary,
                    position: Vec2 { x: 5.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
            ]
        );
        assert_eq!(interaction.focused_node(), Some(node));

        assert_eq!(
            interaction
                .route_pointer(&scene, &primary_release(12.0, 5.0))
                .unwrap(),
            vec![SceneEvent::PointerLeave {
                target: node,
                position: Vec2 { x: 12.0, y: 5.0 },
                modifiers: Modifiers::default(),
            }]
        );

        let secondary_press = PointerEvent::Press {
            button: PointerButton::Secondary,
            position: Vec2 { x: 12.0, y: 5.0 },
            modifiers: Modifiers::default(),
        };
        assert!(
            interaction
                .route_pointer(&scene, &secondary_press)
                .unwrap()
                .is_empty()
        );
        assert_eq!(interaction.focused_node(), Some(node));

        assert_eq!(
            interaction
                .route_pointer(&scene, &primary_press(12.0, 5.0))
                .unwrap(),
            vec![SceneEvent::FocusLost { target: node }]
        );
        assert_eq!(interaction.focused_node(), None);
    }

    #[test]
    fn emits_drag_lifecycle_after_the_logical_distance_threshold() {
        let (scene, source, target) = scene_with_drag_target();
        let mut interaction = SceneInteraction::new();
        interaction
            .route_pointer(&scene, &primary_press(5.0, 5.0))
            .unwrap();

        let below_threshold = DRAG_START_DISTANCE - 0.1;
        assert_eq!(
            interaction
                .route_pointer(&scene, &pointer_move(5.0 + below_threshold, 5.0))
                .unwrap(),
            vec![SceneEvent::PointerMove {
                target: source,
                position: Vec2 {
                    x: 5.0 + below_threshold,
                    y: 5.0,
                },
                modifiers: Modifiers::default(),
            }]
        );
        assert_eq!(interaction.drag_source(), None);

        assert_eq!(
            interaction
                .route_pointer(&scene, &pointer_move(25.0, 5.0))
                .unwrap(),
            vec![
                SceneEvent::PointerLeave {
                    target: source,
                    position: Vec2 { x: 25.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
                SceneEvent::PointerEnter {
                    target,
                    position: Vec2 { x: 25.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
                SceneEvent::PointerMove {
                    target,
                    position: Vec2 { x: 25.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
                SceneEvent::DragStart {
                    source,
                    origin: Vec2 { x: 5.0, y: 5.0 },
                    position: Vec2 { x: 25.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
                SceneEvent::DragEnter {
                    source,
                    target,
                    position: Vec2 { x: 25.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
                SceneEvent::DragMove {
                    source,
                    target: Some(target),
                    position: Vec2 { x: 25.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
            ]
        );
        assert_eq!(interaction.drag_source(), Some(source));

        assert_eq!(
            interaction
                .route_pointer(&scene, &primary_release(25.0, 5.0))
                .unwrap(),
            vec![
                SceneEvent::PointerRelease {
                    target,
                    button: PointerButton::Primary,
                    position: Vec2 { x: 25.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
                SceneEvent::Drop {
                    source,
                    target,
                    position: Vec2 { x: 25.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
                SceneEvent::DragEnd {
                    source,
                    target: Some(target),
                    position: Vec2 { x: 25.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
            ]
        );
        assert_eq!(interaction.drag_source(), None);
    }

    #[test]
    fn starts_at_the_exact_threshold_and_ends_without_a_drop_outside_a_target() {
        let (scene, source, _) = scene_with_drag_target();
        let mut interaction = SceneInteraction::new();
        interaction
            .route_pointer(&scene, &primary_press(5.0, 5.0))
            .unwrap();

        let start_events = interaction
            .route_pointer(&scene, &pointer_move(5.0 + DRAG_START_DISTANCE, 5.0))
            .unwrap();
        assert!(start_events.contains(&SceneEvent::DragStart {
            source,
            origin: Vec2 { x: 5.0, y: 5.0 },
            position: Vec2 {
                x: 5.0 + DRAG_START_DISTANCE,
                y: 5.0,
            },
            modifiers: Modifiers::default(),
        }));
        assert!(start_events.contains(&SceneEvent::DragMove {
            source,
            target: None,
            position: Vec2 {
                x: 5.0 + DRAG_START_DISTANCE,
                y: 5.0,
            },
            modifiers: Modifiers::default(),
        }));

        let end_events = interaction
            .route_pointer(&scene, &primary_release(15.0, 5.0))
            .unwrap();
        assert!(
            !end_events
                .iter()
                .any(|event| matches!(event, SceneEvent::Drop { .. }))
        );
        assert_eq!(
            end_events,
            vec![
                SceneEvent::PointerLeave {
                    target: source,
                    position: Vec2 { x: 15.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
                SceneEvent::DragEnd {
                    source,
                    target: None,
                    position: Vec2 { x: 15.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
            ]
        );
    }

    #[test]
    fn only_the_primary_button_can_begin_a_drag() {
        let (scene, _, target) = scene_with_drag_target();
        let mut interaction = SceneInteraction::new();
        let secondary_press = PointerEvent::Press {
            button: PointerButton::Secondary,
            position: Vec2 { x: 5.0, y: 5.0 },
            modifiers: Modifiers::default(),
        };

        interaction.route_pointer(&scene, &secondary_press).unwrap();
        let events = interaction
            .route_pointer(&scene, &pointer_move(25.0, 5.0))
            .unwrap();

        assert!(events.contains(&SceneEvent::PointerMove {
            target,
            position: Vec2 { x: 25.0, y: 5.0 },
            modifiers: Modifiers::default(),
        }));
        assert!(!events.iter().any(|event| {
            matches!(
                event,
                SceneEvent::DragStart { .. }
                    | SceneEvent::DragEnter { .. }
                    | SceneEvent::DragLeave { .. }
                    | SceneEvent::DragMove { .. }
                    | SceneEvent::Drop { .. }
                    | SceneEvent::DragEnd { .. }
                    | SceneEvent::DragCancel { .. }
            )
        }));
        assert_eq!(interaction.drag_source(), None);
    }

    #[test]
    fn excludes_the_drag_source_and_descendants_but_allows_its_parent() {
        let mut scene = Scene::new();
        let parent = scene.create_root(NodeProperties {
            bounds: Rect::new(0.0, 0.0, 30.0, 30.0).unwrap(),
            ..NodeProperties::default()
        });
        let source = scene
            .create_child(
                parent,
                NodeProperties {
                    bounds: Rect::new(0.0, 0.0, 10.0, 10.0).unwrap(),
                    z_index: 1,
                    ..NodeProperties::default()
                },
            )
            .unwrap();
        scene
            .create_child(
                source,
                NodeProperties {
                    bounds: Rect::new(20.0, 0.0, 10.0, 10.0).unwrap(),
                    ..NodeProperties::default()
                },
            )
            .unwrap();
        let mut interaction = SceneInteraction::new();

        interaction
            .route_pointer(&scene, &primary_press(5.0, 5.0))
            .unwrap();
        let events = interaction
            .route_pointer(&scene, &pointer_move(5.0 + DRAG_START_DISTANCE, 5.0))
            .unwrap();

        assert!(events.contains(&SceneEvent::DragEnter {
            source,
            target: parent,
            position: Vec2 {
                x: 5.0 + DRAG_START_DISTANCE,
                y: 5.0,
            },
            modifiers: Modifiers::default(),
        }));
    }

    #[test]
    fn pointer_exit_keeps_dragging_but_leaves_its_drop_target() {
        let (scene, source, target) = scene_with_drag_target();
        let mut interaction = SceneInteraction::new();
        interaction
            .route_pointer(&scene, &primary_press(5.0, 5.0))
            .unwrap();
        interaction
            .route_pointer(&scene, &pointer_move(25.0, 5.0))
            .unwrap();

        assert_eq!(
            interaction.pointer_left(),
            vec![
                SceneEvent::PointerLeave {
                    target,
                    position: Vec2 { x: 25.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
                SceneEvent::DragLeave {
                    source,
                    target,
                    position: Vec2 { x: 25.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
            ]
        );
        assert_eq!(interaction.drag_source(), Some(source));
    }

    #[test]
    fn window_focus_loss_cancels_an_active_drag() {
        let (scene, source, target) = scene_with_drag_target();
        let mut interaction = SceneInteraction::new();
        interaction
            .route_pointer(&scene, &primary_press(5.0, 5.0))
            .unwrap();
        interaction
            .route_pointer(&scene, &pointer_move(25.0, 5.0))
            .unwrap();

        assert_eq!(
            interaction.window_focus_lost(),
            vec![
                SceneEvent::DragLeave {
                    source,
                    target,
                    position: Vec2 { x: 25.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
                SceneEvent::DragCancel {
                    source,
                    position: Vec2 { x: 25.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
            ]
        );
        assert_eq!(interaction.drag_source(), None);
    }

    #[test]
    fn reconciles_hover_focus_and_drag_when_the_source_is_removed() {
        let (mut scene, source, target) = scene_with_drag_target();
        let mut interaction = SceneInteraction::new();
        interaction
            .route_pointer(&scene, &primary_press(5.0, 5.0))
            .unwrap();
        interaction
            .route_pointer(&scene, &pointer_move(25.0, 5.0))
            .unwrap();
        scene.remove_subtree(source).unwrap();

        assert_eq!(
            interaction.reconcile(&scene).unwrap(),
            vec![
                SceneEvent::FocusLost { target: source },
                SceneEvent::DragLeave {
                    source,
                    target,
                    position: Vec2 { x: 25.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
                SceneEvent::DragCancel {
                    source,
                    position: Vec2 { x: 25.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
            ]
        );
    }

    #[test]
    fn reconciles_hover_and_focus_when_a_node_is_removed() {
        let (mut scene, node) = scene_with_node();
        let mut interaction = SceneInteraction::new();
        interaction
            .route_pointer(&scene, &primary_press(5.0, 5.0))
            .unwrap();
        scene.remove_subtree(node).unwrap();

        assert_eq!(
            interaction.reconcile(&scene).unwrap(),
            vec![
                SceneEvent::PointerLeave {
                    target: node,
                    position: Vec2 { x: 5.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
                SceneEvent::FocusLost { target: node },
            ]
        );
    }

    #[test]
    fn preserves_program_focus_when_the_focused_node_is_hidden() {
        let (mut scene, node) = scene_with_node();
        let mut interaction = SceneInteraction::new();
        let position = Vec2 { x: 4.0, y: 4.0 };

        interaction
            .route_pointer(
                &scene,
                &PointerEvent::Press {
                    button: PointerButton::Primary,
                    position,
                    modifiers: Modifiers::default(),
                },
            )
            .unwrap();
        let mut properties = scene.node(node).unwrap().properties();
        properties.visible = false;
        scene.set_properties(node, properties).unwrap();

        assert_eq!(
            interaction.reconcile(&scene).unwrap(),
            vec![SceneEvent::PointerLeave {
                target: node,
                position,
                modifiers: Modifiers::default(),
            }]
        );
        assert_eq!(interaction.focused_node(), Some(node));
    }

    #[test]
    fn emits_a_leave_when_the_pointer_exits_the_window() {
        let (scene, node) = scene_with_node();
        let mut interaction = SceneInteraction::new();
        interaction
            .route_pointer(&scene, &pointer_move(5.0, 5.0))
            .unwrap();

        assert_eq!(
            interaction.pointer_left(),
            vec![SceneEvent::PointerLeave {
                target: node,
                position: Vec2 { x: 5.0, y: 5.0 },
                modifiers: Modifiers::default(),
            }]
        );
        assert_eq!(interaction.hovered_node(), None);
        assert!(interaction.reconcile(&scene).unwrap().is_empty());
    }

    #[test]
    fn rejects_non_finite_pointer_coordinates() {
        let (scene, _) = scene_with_node();
        let mut interaction = SceneInteraction::new();

        assert_eq!(
            interaction.route_pointer(&scene, &pointer_move(f64::NAN, 0.0)),
            Err(InteractionError::InvalidPointerPosition)
        );
    }
}

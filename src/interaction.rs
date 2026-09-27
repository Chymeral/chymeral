use std::{
    error::Error,
    fmt::{Display, Formatter},
};

use crate::{
    runtime::{Modifiers, PointerButton, PointerEvent, SceneEvent, Vec2},
    scene::{NodeId, Point, Scene, SceneError},
};

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

#[derive(Debug, Default)]
pub struct SceneInteraction {
    hovered_node: Option<NodeId>,
    focused_node: Option<NodeId>,
    pointer_position: Option<Vec2>,
    pointer_modifiers: Modifiers,
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
            }
            PointerEvent::Press { button, .. } => {
                if *button == PointerButton::Primary {
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
        Ok(events)
    }

    pub fn pointer_left(&mut self) -> Vec<SceneEvent> {
        let mut events = Vec::new();
        if let Some(position) = self.pointer_position {
            self.update_hover(None, position, self.pointer_modifiers, &mut events);
        }
        self.pointer_position = None;
        events
    }

    fn target_at(&self, scene: &Scene, position: Vec2) -> Result<Option<NodeId>, InteractionError> {
        let point = Point::new(position.x, position.y)
            .map_err(|_| InteractionError::InvalidPointerPosition)?;
        scene.hit_test(point).map_err(InteractionError::from)
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
            .is_some_and(|node| !scene.paint_order().contains(&node))
        {
            self.update_focus(None, events);
        }
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

#[cfg(test)]
mod tests {
    use crate::{
        runtime::{Modifiers, PointerButton, PointerEvent, SceneEvent, Vec2},
        scene::{NodeId, NodeProperties, Rect, Scene},
    };

    use super::{InteractionError, SceneInteraction};

    fn scene_with_node() -> (Scene, NodeId) {
        let mut scene = Scene::new();
        let node = scene.create_root(NodeProperties {
            bounds: Rect::new(0.0, 0.0, 10.0, 10.0).unwrap(),
            ..NodeProperties::default()
        });
        (scene, node)
    }

    fn pointer_move(x: f64, y: f64) -> PointerEvent {
        PointerEvent::Move {
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
        let press = PointerEvent::Press {
            button: PointerButton::Primary,
            position: Vec2 { x: 5.0, y: 5.0 },
            modifiers: Modifiers::default(),
        };

        assert_eq!(
            interaction.route_pointer(&scene, &press).unwrap(),
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

        let release = PointerEvent::Release {
            button: PointerButton::Primary,
            position: Vec2 { x: 12.0, y: 5.0 },
            modifiers: Modifiers::default(),
        };
        assert_eq!(
            interaction.route_pointer(&scene, &release).unwrap(),
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

        let primary_press = PointerEvent::Press {
            button: PointerButton::Primary,
            position: Vec2 { x: 12.0, y: 5.0 },
            modifiers: Modifiers::default(),
        };
        assert_eq!(
            interaction.route_pointer(&scene, &primary_press).unwrap(),
            vec![SceneEvent::FocusLost { target: node }]
        );
        assert_eq!(interaction.focused_node(), None);
    }

    #[test]
    fn reconciles_hover_and_focus_when_a_node_is_removed() {
        let (mut scene, node) = scene_with_node();
        let mut interaction = SceneInteraction::new();
        interaction
            .route_pointer(
                &scene,
                &PointerEvent::Press {
                    button: PointerButton::Primary,
                    position: Vec2 { x: 5.0, y: 5.0 },
                    modifiers: Modifiers::default(),
                },
            )
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

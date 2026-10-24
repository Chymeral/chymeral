//! Atomic structural CLP scene-batch transaction support.
//!
//! A batch can mutate only retained structural scene state. Image decoding,
//! animation storage, and native window metadata are intentionally excluded:
//! they would require external allocation rollback and are not M26 operations.

use super::*;

/// The retained state that a structural batch may change and must restore on
/// rejection.
pub(super) struct SceneBatchSnapshot {
    scene: Scene,
    next_node_handle: u64,
    nodes: BTreeMap<NodeHandle, OwnedNode>,
    overlap_pairs: BTreeSet<OverlapPair>,
    pending_overlap_events: Vec<SceneEvent>,
}

impl SceneBatchSnapshot {
    pub(super) fn capture(resources: &ProgramSceneResources, scene: &Scene) -> Self {
        Self {
            scene: scene.clone(),
            next_node_handle: resources.next_node_handle,
            nodes: resources.nodes.clone(),
            overlap_pairs: resources.overlap_pairs.clone(),
            pending_overlap_events: resources.pending_overlap_events.clone(),
        }
    }

    pub(super) fn restore(self, resources: &mut ProgramSceneResources, scene: &mut Scene) {
        *scene = self.scene;
        resources.next_node_handle = self.next_node_handle;
        resources.nodes = self.nodes;
        resources.overlap_pairs = self.overlap_pairs;
        resources.pending_overlap_events = self.pending_overlap_events;
    }

    pub(super) fn restore_overlap_state(&self, resources: &mut ProgramSceneResources) {
        resources.overlap_pairs = self.overlap_pairs.clone();
        resources.pending_overlap_events = self.pending_overlap_events.clone();
    }
}

impl ProgramSceneResources {
    /// Applies a bounded structural command sequence without allowing an
    /// incomplete intermediate scene to escape the host event turn.
    ///
    /// Every individual operation still uses the established ownership and
    /// scene validation path. A snapshot restores the retained state and
    /// handle allocator if any operation fails. Overlap state is reset to its
    /// pre-batch snapshot and reconciled once against the final scene, so the
    /// client observes only the atomic transition rather than transient pairs.
    pub(super) fn execute_scene_batch(
        &mut self,
        session: SessionId,
        operations: Vec<SceneBatchOperation>,
        scene: &mut Scene,
        textures: &mut TextureStore,
    ) -> Result<SceneBatchResult, ProgramSceneError> {
        if operations.is_empty() {
            return Err(ProgramSceneError::SceneBatchEmpty);
        }
        if operations.len() > MAX_SCENE_BATCH_OPERATIONS {
            return Err(ProgramSceneError::SceneBatchOperationLimitExceeded {
                limit: MAX_SCENE_BATCH_OPERATIONS,
            });
        }

        let operation_count = operations.len();
        let snapshot = SceneBatchSnapshot::capture(self, scene);
        let result = (|| {
            let mut aliases = BTreeMap::new();
            let mut nodes = BTreeMap::new();
            let mut results = Vec::with_capacity(operation_count);

            for (index, operation) in operations.into_iter().enumerate() {
                let alias = operation.created_alias().map(str::to_owned);
                let result = (|| -> Result<SceneBatchOperationResult, ProgramSceneError> {
                    Ok(match operation {
                        SceneBatchOperation::CreateNode {
                            alias,
                            parent,
                            layer,
                            node,
                        } => {
                            self.validate_scene_batch_alias(&alias)?;
                            if aliases.contains_key(&alias) {
                                return Err(ProgramSceneError::SceneBatchAliasDuplicate(alias));
                            }
                            let parent = parent
                                .as_ref()
                                .map(|parent| {
                                    self.resolve_scene_batch_node(session, parent, &aliases)
                                })
                                .transpose()?;
                            let ClpSceneCommandResult::Node { node, text_metrics } = self
                                .create_clp_node(session, parent, layer, node, scene, textures)?
                            else {
                                unreachable!("creating a CLP node returns a node result")
                            };
                            aliases.insert(alias.clone(), node);
                            nodes.insert(
                                alias.clone(),
                                SceneBatchCreatedNode { node, text_metrics },
                            );
                            SceneBatchOperationResult::Created { alias }
                        }
                        SceneBatchOperation::SetNode { node, changes } => {
                            let node = self.resolve_scene_batch_node(session, &node, &aliases)?;
                            let changes =
                                self.resolve_scene_batch_changes(session, changes, &aliases)?;
                            let ClpSceneCommandResult::NodeUpdated { text_metrics } =
                                self.set_clp_node(session, node, changes, scene, textures)?
                            else {
                                unreachable!("updating a CLP node returns an update result")
                            };
                            SceneBatchOperationResult::Updated { text_metrics }
                        }
                        SceneBatchOperation::RemoveNode { node } => {
                            let node = self.resolve_scene_batch_node(session, &node, &aliases)?;
                            self.execute(
                                session,
                                SceneCommand::RemoveNode { node },
                                scene,
                                textures,
                            )?;
                            SceneBatchOperationResult::Removed
                        }
                    })
                })()
                .map_err(|source| ProgramSceneError::SceneBatchOperation {
                    index,
                    alias,
                    source: Box::new(source),
                })?;
                results.push(result);
            }

            nodes.retain(|_, created| {
                self.nodes.get(&created.node).is_some_and(|owned| {
                    owned.session == session && scene.node(owned.node).is_some()
                })
            });

            Ok(SceneBatchResult {
                nodes,
                operations: results,
            })
        })();

        let result = match result {
            Ok(result) => result,
            Err(error) => {
                snapshot.restore(self, scene);
                return Err(error);
            }
        };

        snapshot.restore_overlap_state(self);
        if let Err(error) = self.reconcile_overlaps(session, scene) {
            snapshot.restore(self, scene);
            return Err(ProgramSceneError::SceneBatchOperation {
                index: operation_count,
                alias: None,
                source: Box::new(error),
            });
        }
        Ok(result)
    }

    fn validate_scene_batch_alias(&self, alias: &str) -> Result<(), ProgramSceneError> {
        if alias.is_empty() {
            return Err(ProgramSceneError::SceneBatchAliasInvalid(
                "it must not be empty",
            ));
        }
        if alias.len() > MAX_SCENE_BATCH_ALIAS_BYTES {
            return Err(ProgramSceneError::SceneBatchAliasInvalid(
                "it exceeds the byte limit",
            ));
        }
        if alias.chars().any(char::is_control) {
            return Err(ProgramSceneError::SceneBatchAliasInvalid(
                "it must not contain control characters",
            ));
        }
        Ok(())
    }

    fn resolve_scene_batch_node(
        &self,
        session: SessionId,
        reference: &SceneBatchNodeRef,
        aliases: &BTreeMap<String, NodeHandle>,
    ) -> Result<NodeHandle, ProgramSceneError> {
        match reference {
            SceneBatchNodeRef::Handle(handle) => {
                self.node_for(session, *handle)?;
                Ok(*handle)
            }
            SceneBatchNodeRef::Alias(alias) => {
                self.validate_scene_batch_alias(alias)?;
                let handle = aliases
                    .get(alias)
                    .copied()
                    .ok_or_else(|| ProgramSceneError::SceneBatchAliasUnknown(alias.clone()))?;
                if self
                    .nodes
                    .get(&handle)
                    .is_some_and(|owned| owned.session == session)
                {
                    Ok(handle)
                } else {
                    Err(ProgramSceneError::SceneBatchAliasRemoved(alias.clone()))
                }
            }
        }
    }

    fn resolve_scene_batch_changes(
        &self,
        session: SessionId,
        changes: SceneBatchNodeChanges,
        aliases: &BTreeMap<String, NodeHandle>,
    ) -> Result<NodeChanges, ProgramSceneError> {
        let parent = changes
            .parent
            .map(|parent| match parent {
                SceneBatchNodeParent::Root => Ok(NodeParent::Root),
                SceneBatchNodeParent::Node(node) => self
                    .resolve_scene_batch_node(session, &node, aliases)
                    .map(NodeParent::Node),
            })
            .transpose()?;
        Ok(NodeChanges {
            properties: changes.properties,
            parent,
            layer: changes.layer,
            visual: changes.visual,
            text: changes.text,
            text_style: changes.text_style,
        })
    }
}

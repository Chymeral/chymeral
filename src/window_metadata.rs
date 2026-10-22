//! Session-owned application window metadata.
//!
//! CLP sessions may claim a title and static icon for the host window. Claims
//! are retained independently so closing or clearing the newest session can
//! deterministically restore the newest remaining live claim.

use std::collections::BTreeMap;

use crate::{program_scene::ImageHandle, protocol::SessionId};

pub const MAX_WINDOW_TITLE_BYTES: usize = 256;
pub const MAX_WINDOW_ICON_DIMENSION: u32 = 256;
pub const MAX_WINDOW_ICON_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum MetadataField<T> {
    #[default]
    Preserve,
    Clear,
    Set(T),
}

impl<T> MetadataField<T> {
    pub const fn is_preserve(&self) -> bool {
        matches!(self, Self::Preserve)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowMetadataPatch {
    pub title: MetadataField<String>,
    pub icon: MetadataField<ImageHandle>,
}

impl WindowMetadataPatch {
    pub const fn is_empty(&self) -> bool {
        self.title.is_preserve() && self.icon.is_preserve()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowMetadata {
    title: Option<String>,
    icon: Option<ImageHandle>,
}

impl WindowMetadata {
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub const fn icon(&self) -> Option<ImageHandle> {
        self.icon
    }

    const fn is_empty(&self) -> bool {
        self.title.is_none() && self.icon.is_none()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowMetadataError {
    EmptyPatch,
    EmptyTitle,
    TitleTooLong { limit: usize },
    TitleContainsControlCharacter,
}

impl std::fmt::Display for WindowMetadataError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyPatch => {
                formatter.write_str("a window metadata update must set or clear at least one field")
            }
            Self::EmptyTitle => formatter.write_str("a window title must not be empty"),
            Self::TitleTooLong { limit } => {
                write!(
                    formatter,
                    "a window title may not exceed {limit} UTF-8 bytes"
                )
            }
            Self::TitleContainsControlCharacter => {
                formatter.write_str("a window title may not contain control characters")
            }
        }
    }
}

impl std::error::Error for WindowMetadataError {}

#[derive(Default)]
pub struct WindowMetadataClaims {
    claims: BTreeMap<SessionId, WindowMetadataClaim>,
    next_order: u64,
}

#[derive(Clone, Debug)]
struct WindowMetadataClaim {
    metadata: WindowMetadata,
    order: u64,
}

impl WindowMetadataClaims {
    pub fn apply(
        &mut self,
        session: SessionId,
        patch: WindowMetadataPatch,
    ) -> Result<(), WindowMetadataError> {
        if patch.is_empty() {
            return Err(WindowMetadataError::EmptyPatch);
        }
        validate_title(&patch.title)?;

        let previous = self.claims.get(&session).map(|claim| &claim.metadata);
        let metadata = WindowMetadata {
            title: apply_field(
                &patch.title,
                previous.and_then(|metadata| metadata.title.clone()),
            ),
            icon: apply_field(&patch.icon, previous.and_then(WindowMetadata::icon)),
        };
        if metadata.is_empty() {
            self.claims.remove(&session);
            return Ok(());
        }

        let order = self.allocate_order();
        self.claims
            .insert(session, WindowMetadataClaim { metadata, order });
        Ok(())
    }

    pub fn active(&self) -> Option<&WindowMetadata> {
        self.claims
            .iter()
            .max_by_key(|(_, claim)| claim.order)
            .map(|(_, claim)| &claim.metadata)
    }

    pub fn remove_session(&mut self, session: SessionId) -> bool {
        self.claims.remove(&session).is_some()
    }

    pub fn references_image(&self, image: ImageHandle) -> bool {
        self.claims
            .values()
            .any(|claim| claim.metadata.icon() == Some(image))
    }

    fn allocate_order(&mut self) -> u64 {
        if self.next_order == u64::MAX {
            self.rebase_orders();
        }
        self.next_order += 1;
        self.next_order
    }

    fn rebase_orders(&mut self) {
        let mut ordered = self
            .claims
            .iter()
            .map(|(session, claim)| (*session, claim.order))
            .collect::<Vec<_>>();
        ordered.sort_by_key(|(session, order)| (*order, *session));
        for (index, (session, _)) in ordered.into_iter().enumerate() {
            if let Some(claim) = self.claims.get_mut(&session) {
                claim.order = u64::try_from(index + 1).unwrap_or(u64::MAX - 1);
            }
        }
        self.next_order = u64::try_from(self.claims.len()).unwrap_or(u64::MAX - 1);
    }
}

fn validate_title(field: &MetadataField<String>) -> Result<(), WindowMetadataError> {
    let MetadataField::Set(title) = field else {
        return Ok(());
    };
    if title.is_empty() {
        return Err(WindowMetadataError::EmptyTitle);
    }
    if title.len() > MAX_WINDOW_TITLE_BYTES {
        return Err(WindowMetadataError::TitleTooLong {
            limit: MAX_WINDOW_TITLE_BYTES,
        });
    }
    if title.chars().any(char::is_control) {
        return Err(WindowMetadataError::TitleContainsControlCharacter);
    }
    Ok(())
}

fn apply_field<T: Clone>(field: &MetadataField<T>, current: Option<T>) -> Option<T> {
    match field {
        MetadataField::Preserve => current,
        MetadataField::Clear => None,
        MetadataField::Set(value) => Some(value.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(value: u64) -> SessionId {
        SessionId::new(value)
    }

    #[test]
    fn newest_claim_wins_and_a_closed_claim_restores_the_previous_one() {
        let mut claims = WindowMetadataClaims::default();
        claims
            .apply(
                session(1),
                WindowMetadataPatch {
                    title: MetadataField::Set("First application".to_owned()),
                    ..WindowMetadataPatch::default()
                },
            )
            .unwrap();
        claims
            .apply(
                session(2),
                WindowMetadataPatch {
                    title: MetadataField::Set("Second application".to_owned()),
                    icon: MetadataField::Set(ImageHandle::new(8)),
                },
            )
            .unwrap();

        assert_eq!(
            claims.active().and_then(WindowMetadata::title),
            Some("Second application")
        );
        assert_eq!(
            claims.active().and_then(WindowMetadata::icon),
            Some(ImageHandle::new(8))
        );

        assert!(claims.remove_session(session(2)));
        assert_eq!(
            claims.active().and_then(WindowMetadata::title),
            Some("First application")
        );
        assert_eq!(claims.active().and_then(WindowMetadata::icon), None);
    }

    #[test]
    fn updating_a_claim_refreshes_its_recency_and_nulls_clear_it() {
        let mut claims = WindowMetadataClaims::default();
        for (session, title) in [(session(1), "First"), (session(2), "Second")] {
            claims
                .apply(
                    session,
                    WindowMetadataPatch {
                        title: MetadataField::Set(title.to_owned()),
                        ..WindowMetadataPatch::default()
                    },
                )
                .unwrap();
        }
        claims
            .apply(
                session(1),
                WindowMetadataPatch {
                    icon: MetadataField::Set(ImageHandle::new(3)),
                    ..WindowMetadataPatch::default()
                },
            )
            .unwrap();
        assert_eq!(
            claims.active().and_then(WindowMetadata::title),
            Some("First")
        );

        claims
            .apply(
                session(1),
                WindowMetadataPatch {
                    title: MetadataField::Clear,
                    icon: MetadataField::Clear,
                },
            )
            .unwrap();
        assert_eq!(
            claims.active().and_then(WindowMetadata::title),
            Some("Second")
        );
    }

    #[test]
    fn rejects_invalid_titles_and_an_empty_patch() {
        let mut claims = WindowMetadataClaims::default();
        assert_eq!(
            claims.apply(session(1), WindowMetadataPatch::default()),
            Err(WindowMetadataError::EmptyPatch)
        );
        assert_eq!(
            claims.apply(
                session(1),
                WindowMetadataPatch {
                    title: MetadataField::Set(String::new()),
                    ..WindowMetadataPatch::default()
                }
            ),
            Err(WindowMetadataError::EmptyTitle)
        );
        assert_eq!(
            claims.apply(
                session(1),
                WindowMetadataPatch {
                    title: MetadataField::Set("line one\nline two".to_owned()),
                    ..WindowMetadataPatch::default()
                }
            ),
            Err(WindowMetadataError::TitleContainsControlCharacter)
        );
    }
}

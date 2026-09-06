//! Typed Home watch events. Wire conversion stays in this crate.

use mediaops_core::{HomeError, HomeObject};

use crate::home::{WatchResponse, WatchType};
use crate::home_object_from_wire;

/// Decoded watch event. Unspecified/unknown types are errors, not a variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    Added(HomeObject),
    Modified(HomeObject),
    Deleted(HomeObject),
}

impl WatchEvent {
    pub fn object(&self) -> &HomeObject {
        match self {
            Self::Added(obj) | Self::Modified(obj) | Self::Deleted(obj) => obj,
        }
    }
}

/// Decode a wire `WatchResponse` into a domain event.
pub fn watch_event_from_wire(resp: WatchResponse) -> Result<WatchEvent, HomeError> {
    let obj = resp
        .object
        .ok_or_else(|| HomeError::Invalid("watch event missing object".into()))?;
    let obj = home_object_from_wire(obj)?;
    let ty = WatchType::try_from(resp.r#type)
        .map_err(|_| HomeError::Invalid(format!("unknown watch type {}", resp.r#type)))?;
    match ty {
        WatchType::Added => Ok(WatchEvent::Added(obj)),
        WatchType::Modified => Ok(WatchEvent::Modified(obj)),
        WatchType::Deleted => Ok(WatchEvent::Deleted(obj)),
        WatchType::Unspecified => Err(HomeError::Invalid("unspecified watch type".into())),
    }
}

#[cfg(test)]
mod tests {
    use mediaops_core::{HomeObject, Kind, Spec, StatusBody, WantSpec, WantStatus};

    use super::*;
    use crate::home_object_to_wire;

    fn want() -> HomeObject {
        HomeObject::new(
            Kind::Want,
            "movie:tmdb:603",
            Spec::Want(WantSpec {
                title_id: "movie:tmdb:603".into(),
            }),
            StatusBody::Want(WantStatus::default()),
        )
    }

    #[test]
    fn added_modified_deleted_decode() {
        let obj = want();
        for (ty, pred) in [
            (
                WatchType::Added,
                WatchEvent::Added as fn(HomeObject) -> WatchEvent,
            ),
            (
                WatchType::Modified,
                WatchEvent::Modified as fn(HomeObject) -> WatchEvent,
            ),
            (
                WatchType::Deleted,
                WatchEvent::Deleted as fn(HomeObject) -> WatchEvent,
            ),
        ] {
            let ev = watch_event_from_wire(WatchResponse {
                r#type: ty as i32,
                object: Some(home_object_to_wire(&obj)),
            })
            .expect("decode");
            assert_eq!(ev, pred(obj.clone()));
        }
    }

    #[test]
    fn unspecified_and_missing_object_are_invalid() {
        let obj = want();
        assert!(matches!(
            watch_event_from_wire(WatchResponse {
                r#type: WatchType::Unspecified as i32,
                object: Some(home_object_to_wire(&obj)),
            }),
            Err(HomeError::Invalid(_))
        ));
        assert!(matches!(
            watch_event_from_wire(WatchResponse {
                r#type: WatchType::Added as i32,
                object: None,
            }),
            Err(HomeError::Invalid(_))
        ));
        assert!(matches!(
            watch_event_from_wire(WatchResponse {
                r#type: 99,
                object: Some(home_object_to_wire(&obj)),
            }),
            Err(HomeError::Invalid(_))
        ));
    }
}

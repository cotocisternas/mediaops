use mediaops_core::{HoldDecisionSpec, HomeObject, Kind, Spec, StatusBody, TitleId, WantSpec};
use mediaops_home_client::{ClientError, HomeApi};

use crate::cache::{ObjectCache, ObjectKey};
use crate::inventory::committed_inventory_generation;
use crate::model::Screen;
use crate::subscription::RPC_LIMIT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutation {
    ApplyWant,
    DeleteWant,
    ApproveHold,
    RejectHold,
}

impl Mutation {
    pub const fn allowed_on(self, screen: Screen) -> bool {
        match self {
            Self::ApplyWant => matches!(screen, Screen::Wants | Screen::Titles),
            Self::DeleteWant => matches!(screen, Screen::Wants),
            Self::ApproveHold | Self::RejectHold => matches!(screen, Screen::Holds),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationTarget {
    pub key: ObjectKey,
    pub uid: String,
    pub resource_version: i64,
    pub epoch: u64,
}

impl MutationTarget {
    pub fn matches_cache(&self, cache: &ObjectCache) -> bool {
        if self.epoch != cache.epoch() {
            return false;
        }
        match cache.get(&self.key).and_then(|entry| entry.object.as_ref()) {
            Some(obj) => self.matches_object(obj),
            None => {
                self.key.kind == Kind::Want && self.resource_version == 0 && self.uid.is_empty()
            }
        }
    }

    pub fn matches_object(&self, obj: &HomeObject) -> bool {
        self.key == ObjectKey::from_object(obj)
            && self.uid == obj.metadata.uid
            && self.resource_version == obj.metadata.resource_version
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationOutcome {
    Applied,
    Deleted,
    Conflict,
    Unknown,
    Unavailable,
    PreflightFailed(String),
    Rejected(String),
}

impl MutationOutcome {
    pub fn message(&self) -> String {
        match self {
            Self::Applied => "applied".into(),
            Self::Deleted => "deleted".into(),
            Self::Conflict => "object changed; refreshing (no retry)".into(),
            Self::Unknown => "outcome unknown; refreshing (not resent)".into(),
            Self::Unavailable => "action unavailable; no write submitted".into(),
            Self::PreflightFailed(message) => format!("no write submitted: {message}"),
            Self::Rejected(message) => format!("rejected: {message}"),
        }
    }
}

pub enum PreparedWrite {
    Apply(HomeObject),
    Delete(HomeObject),
    Decide(HomeObject),
}

pub async fn prepare(
    api: &HomeApi,
    mutation: Mutation,
    target: &MutationTarget,
) -> Result<PreparedWrite, MutationOutcome> {
    match mutation {
        Mutation::ApplyWant | Mutation::DeleteWant => {
            if target.key.kind != Kind::Want || TitleId::parse(&target.key.name).is_err() {
                return Err(MutationOutcome::Unavailable);
            }
            let object = match read(api.get(Kind::Want, &target.key.name)).await {
                Ok(obj) if target.matches_object(&obj) => obj,
                Ok(_) => return Err(MutationOutcome::Conflict),
                Err(ReadFailure::NotFound)
                    if mutation == Mutation::ApplyWant
                        && target.uid.is_empty()
                        && target.resource_version == 0 =>
                {
                    HomeObject::new(
                        Kind::Want,
                        target.key.name.clone(),
                        Spec::Want(WantSpec {
                            title_id: target.key.name.clone(),
                        }),
                        StatusBody::empty(Kind::Want),
                    )
                }
                Err(ReadFailure::NotFound) => return Err(MutationOutcome::Conflict),
                Err(ReadFailure::Failed(message)) => {
                    return Err(MutationOutcome::PreflightFailed(message));
                }
            };
            Ok(if mutation == Mutation::ApplyWant {
                PreparedWrite::Apply(object)
            } else {
                PreparedWrite::Delete(object)
            })
        }
        Mutation::ApproveHold | Mutation::RejectHold => {
            if target.key.kind != Kind::Hold {
                return Err(MutationOutcome::Unavailable);
            }
            let objects = read(api.list(None))
                .await
                .map_err(|err| MutationOutcome::PreflightFailed(err.message()))?;
            let generation =
                committed_inventory_generation(objects.iter(), crate::clock::unix_now())
                    .ok_or(MutationOutcome::Unavailable)?;
            let mut hold = objects
                .into_iter()
                .find(|obj| target.matches_object(obj))
                .ok_or(MutationOutcome::Conflict)?;
            if !matches!(&hold.status, StatusBody::Hold(st) if st.list_generation == generation) {
                return Err(MutationOutcome::Conflict);
            }
            let Spec::Hold(spec) = &mut hold.spec else {
                return Err(MutationOutcome::Unavailable);
            };
            if spec.decision != HoldDecisionSpec::Empty {
                return Err(MutationOutcome::Conflict);
            }
            spec.decision = match mutation {
                Mutation::ApproveHold => HoldDecisionSpec::Approved,
                Mutation::RejectHold => HoldDecisionSpec::Rejected,
                Mutation::ApplyWant | Mutation::DeleteWant => {
                    return Err(MutationOutcome::Unavailable);
                }
            };
            Ok(PreparedWrite::Decide(hold))
        }
    }
}

pub async fn submit(api: &HomeApi, prepared: PreparedWrite) -> MutationOutcome {
    let deleted = matches!(prepared, PreparedWrite::Delete(_));
    let request = async {
        match prepared {
            PreparedWrite::Apply(obj) => api.apply(obj).await,
            PreparedWrite::Delete(obj) => {
                api.delete_at_version(
                    Kind::Want,
                    &obj.metadata.name,
                    obj.metadata.resource_version,
                )
                .await
            }
            PreparedWrite::Decide(obj) => api.patch(obj, "spec").await,
        }
    };
    match tokio::time::timeout(RPC_LIMIT, request).await {
        Ok(Ok(_)) => {
            if deleted {
                MutationOutcome::Deleted
            } else {
                MutationOutcome::Applied
            }
        }
        Ok(Err(err)) if err.is_conflict() || err.is_not_found() => MutationOutcome::Conflict,
        Ok(Err(err)) if err.is_uncertain() => MutationOutcome::Unknown,
        Ok(Err(err)) => MutationOutcome::Rejected(err.to_string()),
        Err(_) => MutationOutcome::Unknown,
    }
}

pub async fn execute(
    api: &HomeApi,
    mutation: Mutation,
    target: &MutationTarget,
    cache: &ObjectCache,
    _now_unix: i64,
) -> MutationOutcome {
    if !target.matches_cache(cache) {
        return MutationOutcome::Conflict;
    }
    match prepare(api, mutation, target).await {
        Ok(prepared) => submit(api, prepared).await,
        Err(outcome) => outcome,
    }
}

enum ReadFailure {
    NotFound,
    Failed(String),
}
impl ReadFailure {
    fn message(self) -> String {
        match self {
            Self::NotFound => "not found".into(),
            Self::Failed(message) => message,
        }
    }
}
async fn read<T>(future: impl Future<Output = Result<T, ClientError>>) -> Result<T, ReadFailure> {
    match tokio::time::timeout(RPC_LIMIT, future).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) if err.is_not_found() => Err(ReadFailure::NotFound),
        Ok(Err(err)) => Err(ReadFailure::Failed(err.to_string())),
        Err(_) => Err(ReadFailure::Failed("Home API request timed out".into())),
    }
}

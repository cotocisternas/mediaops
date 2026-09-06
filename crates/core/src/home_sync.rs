//! One-shot Sync object types. Planning and persistence live in the API.

use serde::{Deserialize, Serialize};

use crate::home::{ClusterSpec, HomeError, JobSpec};
use crate::pathschema::Placement;

fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}

/// Fixed copy scope for a Sync request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncScope {
    #[default]
    AllEligibleCompleted,
}

impl SyncScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AllEligibleCompleted => "allEligibleCompleted",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, HomeError> {
        match raw {
            "" | "allEligibleCompleted" => Ok(Self::AllEligibleCompleted),
            other => Err(HomeError::Invalid(format!("unknown Sync scope `{other}`"))),
        }
    }
}

/// Empty intent beyond the fixed scope.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncSpec {
    #[serde(default)]
    pub scope: SyncScope,
}

/// Planning phase. A Scheduled Sync never resumes planning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncPhase {
    #[default]
    WaitingInventory,
    Captured,
    Scheduled,
    Failed,
}

impl SyncPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WaitingInventory => "waitingInventory",
            Self::Captured => "captured",
            Self::Scheduled => "scheduled",
            Self::Failed => "failed",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, HomeError> {
        match raw {
            "" | "waitingInventory" => Ok(Self::WaitingInventory),
            "captured" => Ok(Self::Captured),
            "scheduled" => Ok(Self::Scheduled),
            "failed" => Ok(Self::Failed),
            other => Err(HomeError::Invalid(format!("unknown Sync phase `{other}`"))),
        }
    }
}

/// Per-file planner decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncDisposition {
    #[default]
    WouldQueue,
    Queued,
    AlreadyQueued,
    Present,
    Blocked,
    Ineligible,
}

impl SyncDisposition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WouldQueue => "wouldQueue",
            Self::Queued => "queued",
            Self::AlreadyQueued => "alreadyQueued",
            Self::Present => "present",
            Self::Blocked => "blocked",
            Self::Ineligible => "ineligible",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, HomeError> {
        match raw {
            "" | "wouldQueue" => Ok(Self::WouldQueue),
            "queued" => Ok(Self::Queued),
            "alreadyQueued" => Ok(Self::AlreadyQueued),
            "present" => Ok(Self::Present),
            "blocked" => Ok(Self::Blocked),
            "ineligible" => Ok(Self::Ineligible),
            other => Err(HomeError::Invalid(format!(
                "unknown Sync disposition `{other}`"
            ))),
        }
    }
}

/// One captured source file and its decision.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncEntry {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remote_root: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remote_path: String,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub file_len: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<Placement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<JobSpec>,
    #[serde(default)]
    pub disposition: SyncDisposition,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub job_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub job_uid: String,
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

/// Server-owned Sync manifest and planning progress.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncStatus {
    #[serde(default)]
    pub phase: SyncPhase,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub accepted_unix: i64,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub deadline_unix: i64,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub baseline_generation: i64,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub acceptance_rv: i64,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub list_generation: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster: Option<ClusterSpec>,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub cluster_generation: i64,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub secret_resource_version: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<SyncEntry>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_and_disposition_tokens_round_trip() {
        for phase in [
            SyncPhase::WaitingInventory,
            SyncPhase::Captured,
            SyncPhase::Scheduled,
            SyncPhase::Failed,
        ] {
            assert_eq!(SyncPhase::parse(phase.as_str()).expect("phase"), phase);
        }
        for disposition in [
            SyncDisposition::WouldQueue,
            SyncDisposition::Queued,
            SyncDisposition::AlreadyQueued,
            SyncDisposition::Present,
            SyncDisposition::Blocked,
            SyncDisposition::Ineligible,
        ] {
            assert_eq!(
                SyncDisposition::parse(disposition.as_str()).expect("disposition"),
                disposition
            );
        }
        assert_eq!(
            SyncScope::parse(SyncScope::AllEligibleCompleted.as_str()).expect("scope"),
            SyncScope::AllEligibleCompleted
        );
    }

    #[test]
    fn default_sync_json_omits_empty_and_zero_fields() {
        let status = SyncStatus::default();
        let value = serde_json::to_value(&status).expect("json");
        let object = value.as_object().expect("object");
        assert_eq!(
            object.get("phase").and_then(|v| v.as_str()),
            Some("waitingInventory")
        );
        assert!(!object.contains_key("acceptedUnix"));
        assert!(!object.contains_key("entries"));
        assert!(!object.contains_key("cluster"));
        let entry = serde_json::to_value(SyncEntry::default()).expect("entry");
        let entry = entry.as_object().expect("entry object");
        assert_eq!(
            entry.get("disposition").and_then(|v| v.as_str()),
            Some("wouldQueue")
        );
        assert!(!entry.contains_key("job"));
        assert!(!entry.contains_key("fileLen"));
    }

    #[test]
    fn unknown_tokens_are_invalid() {
        assert!(SyncPhase::parse("pending").is_err());
        assert!(SyncDisposition::parse("skip").is_err());
        assert!(SyncScope::parse("everything").is_err());
    }
}

//! Plan artifact: exact desired-state TOML bytes plus `blake3(bytes)`.

use serde::{Deserialize, Serialize};

use crate::desired_state::{DesiredState, DesiredStateError};

/// Exhaustive plan action. Match every variant; do not add a `_` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    Copy,
    Skip,
    Review,
    Unmonitor,
    DeleteRemote,
    Encode,
    Reclaim,
    EdgeApply,
    GrabApply,
}

/// JSON plan. `desired_state_toml` is the exact snapshotted TOML text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    desired_state_toml: Vec<u8>,
    desired_state_b3: String,
    actions: Vec<Action>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanJson {
    desired_state_toml: String,
    desired_state_b3: String,
    actions: Vec<Action>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("desired-state snapshot is not valid UTF-8")]
    InvalidUtf8,
    #[error(transparent)]
    DesiredState(#[from] DesiredStateError),
    #[error("desired_state_b3 must be 64 lowercase hex characters")]
    InvalidDigest,
    #[error("desired_state_b3 does not match blake3 of embedded bytes")]
    DigestMismatch,
}

impl Plan {
    pub fn from_toml_bytes(bytes: impl Into<Vec<u8>>) -> Result<Self, PlanError> {
        let desired_state_toml = bytes.into();
        let text = std::str::from_utf8(&desired_state_toml).map_err(|_| PlanError::InvalidUtf8)?;
        DesiredState::from_toml(text)?;
        Ok(Self {
            desired_state_b3: blake3_hex(&desired_state_toml),
            desired_state_toml,
            actions: Vec::new(),
        })
    }

    pub fn with_actions(mut self, actions: Vec<Action>) -> Self {
        self.actions = actions;
        self
    }

    pub fn desired_state_toml(&self) -> &[u8] {
        &self.desired_state_toml
    }

    pub fn desired_state_b3(&self) -> &str {
        &self.desired_state_b3
    }

    pub fn actions(&self) -> &[Action] {
        &self.actions
    }

    /// Re-parse DesiredState only from the embedded bytes.
    pub fn desired_state(&self) -> Result<DesiredState, DesiredStateError> {
        DesiredState::from_toml_bytes(&self.desired_state_toml)
    }

    /// Bytes-hash vs bytes-hash. Does not parse `active_toml`.
    pub fn matches_snapshot(&self, active_toml: &[u8]) -> bool {
        blake3_hex(active_toml) == self.desired_state_b3
    }
}

impl Serialize for Plan {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let desired_state_toml =
            std::str::from_utf8(&self.desired_state_toml).map_err(serde::ser::Error::custom)?;
        PlanJson {
            desired_state_toml: desired_state_toml.to_string(),
            desired_state_b3: self.desired_state_b3.clone(),
            actions: self.actions.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Plan {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let json = PlanJson::deserialize(deserializer)?;
        Plan::from_json(json).map_err(serde::de::Error::custom)
    }
}

impl Plan {
    fn from_json(json: PlanJson) -> Result<Self, PlanError> {
        if !is_lowercase_b3_hex(&json.desired_state_b3) {
            return Err(PlanError::InvalidDigest);
        }
        let desired_state_toml = json.desired_state_toml.into_bytes();
        DesiredState::from_toml_bytes(&desired_state_toml)?;
        if blake3_hex(&desired_state_toml) != json.desired_state_b3 {
            return Err(PlanError::DigestMismatch);
        }
        Ok(Self {
            desired_state_toml,
            desired_state_b3: json.desired_state_b3,
            actions: json.actions,
        })
    }
}

fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn is_lowercase_b3_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desired_state::{DesiredState, HAPPY_TOML};

    #[test]
    fn plan_round_trip_preserves_bytes_and_digest() {
        let bytes = HAPPY_TOML.as_bytes().to_vec();
        let expected_digest = blake3::hash(&bytes).to_hex().to_string();
        assert_eq!(expected_digest.len(), 64);
        assert!(
            expected_digest
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        );

        let plan = Plan::from_toml_bytes(bytes.clone())
            .expect("snapshot")
            .with_actions(vec![Action::Copy, Action::Encode]);
        assert_eq!(plan.desired_state_toml(), bytes.as_slice());
        assert_eq!(plan.desired_state_b3(), expected_digest);

        let json = serde_json::to_string(&plan).expect("serialize");
        let value: serde_json::Value = serde_json::from_str(&json).expect("value");
        assert_eq!(value["desired_state_toml"].as_str(), Some(HAPPY_TOML));
        assert_eq!(value["desired_state_b3"], expected_digest);

        let decoded: Plan = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded.desired_state_toml(), bytes.as_slice());
        assert_eq!(decoded.desired_state_b3(), expected_digest);
        assert_eq!(decoded.actions(), &[Action::Copy, Action::Encode]);
        assert_eq!(decoded, plan);

        let from_embedding = decoded.desired_state().expect("re-parse");
        let from_source = DesiredState::from_toml_bytes(&bytes).expect("source");
        assert_eq!(from_embedding, from_source);
    }

    #[test]
    fn matches_snapshot_is_bytes_hash_equality() {
        let plan = Plan::from_toml_bytes(HAPPY_TOML.as_bytes()).expect("snapshot");
        assert!(plan.matches_snapshot(HAPPY_TOML.as_bytes()));

        let other = HAPPY_TOML.replace("lock = true", "lock = false");
        assert_ne!(other.as_bytes(), HAPPY_TOML.as_bytes());
        assert!(!plan.matches_snapshot(other.as_bytes()));
        // Refuse: still re-parse only from the embedding, never from the active bytes.
        let embedded = plan.desired_state().expect("embedded");
        let active = DesiredState::from_toml(&other).expect("active");
        assert_ne!(embedded, active);
        assert_eq!(
            embedded,
            DesiredState::from_toml_bytes(plan.desired_state_toml()).expect("from plan bytes")
        );
    }

    #[test]
    fn matches_snapshot_rejects_byte_difference_with_same_parse() {
        let plan = Plan::from_toml_bytes(HAPPY_TOML.as_bytes()).expect("snapshot");
        let equivalent = HAPPY_TOML.replace(" = ", "=");
        let parsed_original = DesiredState::from_toml(HAPPY_TOML).expect("original");
        let parsed_equivalent = DesiredState::from_toml(&equivalent).expect("equivalent");
        assert_eq!(parsed_original, parsed_equivalent);
        assert_ne!(equivalent.as_bytes(), HAPPY_TOML.as_bytes());
        assert!(!plan.matches_snapshot(equivalent.as_bytes()));
    }

    #[test]
    fn action_match_is_exhaustive() {
        let actions = [
            Action::Copy,
            Action::Skip,
            Action::Review,
            Action::Unmonitor,
            Action::DeleteRemote,
            Action::Encode,
            Action::Reclaim,
            Action::EdgeApply,
            Action::GrabApply,
        ];
        for action in actions {
            let _ = match action {
                Action::Copy => "copy",
                Action::Skip => "skip",
                Action::Review => "review",
                Action::Unmonitor => "unmonitor",
                Action::DeleteRemote => "delete_remote",
                Action::Encode => "encode",
                Action::Reclaim => "reclaim",
                Action::EdgeApply => "edge_apply",
                Action::GrabApply => "grab_apply",
            };
        }
        assert_eq!(actions.len(), 9);
    }

    #[test]
    fn action_json_tokens_are_pascal_case() {
        let cases = [
            (Action::Copy, "Copy"),
            (Action::Skip, "Skip"),
            (Action::Review, "Review"),
            (Action::Unmonitor, "Unmonitor"),
            (Action::DeleteRemote, "DeleteRemote"),
            (Action::Encode, "Encode"),
            (Action::Reclaim, "Reclaim"),
            (Action::EdgeApply, "EdgeApply"),
            (Action::GrabApply, "GrabApply"),
        ];
        for (action, token) in cases {
            let encoded = serde_json::to_string(&action).expect("serialize");
            assert_eq!(encoded, format!("\"{token}\""));
        }
    }

    #[test]
    fn extra_json_key_is_denied() {
        let mut value = plan_json_value();
        value["extra"] = serde_json::Value::Bool(true);
        let err = serde_json::from_value::<Plan>(value).expect_err("unknown field");
        assert!(
            err.to_string().contains("unknown field"),
            "expected deny_unknown_fields, got {err}"
        );
    }

    #[test]
    fn invalid_utf8_is_an_error() {
        assert_eq!(
            Plan::from_toml_bytes(vec![0xff]),
            Err(PlanError::InvalidUtf8)
        );
    }

    #[test]
    fn uppercase_digest_is_invalid() {
        let mut value = plan_json_value();
        let hex = value["desired_state_b3"].as_str().expect("hex").to_string();
        value["desired_state_b3"] = serde_json::Value::String(hex.to_ascii_uppercase());
        let err = serde_json::from_value::<Plan>(value).expect_err("uppercase");
        assert!(
            err.to_string().contains("64 lowercase hex"),
            "expected InvalidDigest, got {err}"
        );
    }

    #[test]
    fn wrong_length_digest_is_invalid() {
        let mut value = plan_json_value();
        value["desired_state_b3"] = serde_json::Value::String("abc".into());
        let err = serde_json::from_value::<Plan>(value).expect_err("short digest");
        assert!(
            err.to_string().contains("64 lowercase hex"),
            "expected InvalidDigest, got {err}"
        );
    }

    fn plan_json_value() -> serde_json::Value {
        let plan = Plan::from_toml_bytes(HAPPY_TOML.as_bytes()).expect("snapshot");
        serde_json::from_str(&serde_json::to_string(&plan).expect("json")).expect("value")
    }

    #[test]
    fn json_digest_mismatch_is_refused() {
        let plan = Plan::from_toml_bytes(HAPPY_TOML.as_bytes()).expect("snapshot");
        let mut value: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&plan).expect("json")).expect("value");
        value["desired_state_b3"] = serde_json::Value::String("0".repeat(64));
        let err = serde_json::from_value::<Plan>(value).expect_err("mismatch");
        assert!(
            err.to_string().contains("does not match blake3"),
            "got {err}"
        );
    }
}

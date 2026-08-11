//! The recording lifecycle state machine described in the plan. Exactly one
//! session may be `Preparing`, `Recording`, `Recovering`, `Stopping`, or
//! `Finalizing` at a time; `SessionManager` (see `manager.rs`) enforces that
//! invariant and is the only writer of this state.

use serde::Serialize;

use crate::audio::MeetingAppId;
use crate::error::AppError;
use crate::storage::session_store::SessionMetadata;

// NOTE: `rename_all` on an enum only renames the variant tag (the `state`
// value below), not the fields nested inside each struct-like variant --
// those need their own `rename_all` per variant, or they serialize with
// their original Rust (snake_case) names while the frontend expects
// camelCase, silently producing `undefined` for fields like `sessionId`
// (which then vanishes entirely once passed back into an `invoke()` call,
// since JSON serialization drops `undefined`-valued keys).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum SessionState {
    Idle,
    #[serde(rename_all = "camelCase")]
    Preparing {
        session_id: String,
    },
    #[serde(rename_all = "camelCase")]
    Recording {
        session_id: String,
        started_at_ms: i64,
        mic_enabled: bool,
        source_app: MeetingAppId,
    },
    #[serde(rename_all = "camelCase")]
    Recovering {
        session_id: String,
        started_at_ms: i64,
        mic_enabled: bool,
        source_app: MeetingAppId,
    },
    #[serde(rename_all = "camelCase")]
    Stopping {
        session_id: String,
    },
    #[serde(rename_all = "camelCase")]
    Finalizing {
        session_id: String,
    },
    Completed {
        metadata: SessionMetadata,
    },
    Failed {
        error: AppError,
    },
}

impl Default for SessionState {
    fn default() -> Self {
        SessionState::Idle
    }
}

impl SessionState {
    pub fn session_id(&self) -> Option<&str> {
        match self {
            SessionState::Preparing { session_id }
            | SessionState::Recording { session_id, .. }
            | SessionState::Recovering { session_id, .. }
            | SessionState::Stopping { session_id }
            | SessionState::Finalizing { session_id } => Some(session_id),
            _ => None,
        }
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            SessionState::Preparing { .. }
                | SessionState::Recording { .. }
                | SessionState::Recovering { .. }
                | SessionState::Stopping { .. }
                | SessionState::Finalizing { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test for a real bug: `sessionState.sessionId` read
    /// `undefined` in the frontend because this enum's fields serialized as
    /// snake_case (`session_id`) despite the container-level
    /// `rename_all = "camelCase")`, which only renames the `state` tag, not
    /// fields nested in struct variants. `stopRecording(undefined)` then
    /// silently dropped the key entirely on the way to `invoke()`, and the
    /// Rust command handler failed with "missing required key sessionId".
    #[test]
    fn recording_variant_serializes_field_names_as_camel_case() {
        let state = SessionState::Recording {
            session_id: "abc-123".to_string(),
            started_at_ms: 1_700_000_000_000,
            mic_enabled: true,
            source_app: MeetingAppId::Zoom,
        };
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["state"], "recording");
        assert_eq!(json["sessionId"], "abc-123");
        assert_eq!(json["startedAtMs"], 1_700_000_000_000i64);
        assert_eq!(json["micEnabled"], true);
        assert_eq!(json["sourceApp"], "zoom");
        assert!(
            json.get("session_id").is_none(),
            "must not also emit the snake_case field name"
        );
    }

    #[test]
    fn busy_variants_all_serialize_session_id_as_camel_case() {
        let variants = [
            SessionState::Preparing {
                session_id: "s".to_string(),
            },
            SessionState::Recovering {
                session_id: "s".to_string(),
                started_at_ms: 1,
                mic_enabled: false,
                source_app: MeetingAppId::Teams,
            },
            SessionState::Stopping {
                session_id: "s".to_string(),
            },
            SessionState::Finalizing {
                session_id: "s".to_string(),
            },
        ];
        for variant in variants {
            let json = serde_json::to_value(&variant).unwrap();
            assert_eq!(json["sessionId"], "s", "failed for {json:?}");
        }
    }
}

//! RPC request/response types.
//!
//! Spec: prompts/docs/rpc-protocol.md, docs/05-agent-rpc-and-lifecycle.md
//!
//! Wire format: [u32 LE length][UTF-8 JSON body]
//! Requests carry an "op" tag for operation dispatch.

use serde::{Deserialize, Serialize};

// ── Health ────────────────────────────────────────────────────────────────────

/// Braced-struct form: serialises to `{}` (not `null`), forward-compatible.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthRequest {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub ok: bool,
}

// ── CreateSession ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub uuid: String,
    pub shortname: String,
    pub repo_slug: String,
    pub branch: String,
    pub workdir_parent: String, // "<home>/.mux/<uuid>"
    pub repo_leaf: String,      // final component of the repo path
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateSessionResponse {
    pub uuid: String,
    pub shortname: String,
    pub tmux_name: String, // "mux-<shortname>"
}

// ── ListSessions ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListSessionsRequest {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub uuid: String,
    pub shortname: String,
    pub tmux_name: String,
    pub workdir: String,
    pub status: SessionStatusValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListSessionsResponse {
    pub sessions: Vec<SessionInfo>,
}

// ── GetSession ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GetSessionRequest {
    pub uuid: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GetSessionResponse {
    pub uuid: String,
    pub shortname: String,
    pub tmux_name: String,
    pub status: SessionStatusValue,
}

// ── KillSession ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KillSessionRequest {
    pub uuid: String,
    pub repo_slug: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KillSessionResponse {
    pub tmux_killed: bool,
    pub workdir_removed: bool,
}

// ── Shutdown ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShutdownRequest {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShutdownResponse {}

// ── StreamSessionEvents (unimplemented in v0.1) ───────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamSessionEventsRequest {}

// No StreamSessionEventsResponse: always returns RpcError { error: "internal", message: "streaming not implemented" }

// ── Session status ────────────────────────────────────────────────────────────

/// Session status values used in RPC responses.
///
/// Snake-case on the wire to match the local SQLite SessionStatus strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatusValue {
    Active,
    Dead,
    Unreachable,
    Orphaned,
}

// ── Error response ────────────────────────────────────────────────────────────

/// All RPC operations may return an error instead of the expected response.
///
/// Defined error keys (from rpc-protocol.md):
/// - `"not_owned"` — KillSession: uuid not in ownership map
/// - `"not_found"` — GetSession: uuid unknown
/// - `"tmux_error"` — any operation where tmux command fails
/// - `"internal"` — unexpected errors; also for unimplemented operations
/// - `"agent_start_timeout"` — client-side, pre-connection
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub error: String,
    pub message: String,
}

impl RpcError {
    pub fn not_owned(message: impl Into<String>) -> Self {
        Self {
            error: "not_owned".to_owned(),
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            error: "not_found".to_owned(),
            message: message.into(),
        }
    }

    pub fn tmux_error(message: impl Into<String>) -> Self {
        Self {
            error: "tmux_error".to_owned(),
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            error: "internal".to_owned(),
            message: message.into(),
        }
    }

    pub fn agent_start_timeout(message: impl Into<String>) -> Self {
        Self {
            error: "agent_start_timeout".to_owned(),
            message: message.into(),
        }
    }
}

// ── Request envelope ──────────────────────────────────────────────────────────

/// All requests are tagged with `"op"` for server-side dispatch.
///
/// Serialises as `{ "op": "Health" }`, `{ "op": "GetSession", "uuid": "..." }`, etc.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum Request {
    Health(HealthRequest),
    CreateSession(CreateSessionRequest),
    ListSessions(ListSessionsRequest),
    GetSession(GetSessionRequest),
    KillSession(KillSessionRequest),
    Shutdown(ShutdownRequest),
    StreamSessionEvents(StreamSessionEventsRequest),
}

// ── Response envelopes ────────────────────────────────────────────────────────

/// A response that may be either the expected type T or an RpcError.
///
/// Serialised with untagged representation: T fields appear directly, or
/// `{ "error": "...", "message": "..." }` for errors.
///
/// IMPORTANT: T must not have an `"error"` field or the untagged discrimination
/// will be ambiguous. All current response types satisfy this constraint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RpcResult<T> {
    Ok(T),
    Err(RpcError),
}

impl<T> RpcResult<T> {
    pub fn into_result(self) -> Result<T, RpcError> {
        match self {
            RpcResult::Ok(v) => Ok(v),
            RpcResult::Err(e) => Err(e),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_request_serialises_to_empty_object() {
        let req = HealthRequest {};
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, "{}");
    }

    #[test]
    fn health_response_roundtrip() {
        let resp = HealthResponse { ok: true };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: HealthResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn create_session_request_roundtrip() {
        let req = CreateSessionRequest {
            uuid: "abc-123".to_owned(),
            shortname: "my-session".to_owned(),
            repo_slug: "my-repo".to_owned(),
            branch: "main".to_owned(),
            workdir_parent: "/home/user/.mux/abc-123".to_owned(),
            repo_leaf: "my-repo".to_owned(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: CreateSessionRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, decoded);
    }

    #[test]
    fn list_sessions_response_with_sessions_roundtrip() {
        let resp = ListSessionsResponse {
            sessions: vec![
                SessionInfo {
                    uuid: "abc-123".to_owned(),
                    shortname: "session-a".to_owned(),
                    tmux_name: "mux-session-a".to_owned(),
                    workdir: "/home/user/.mux/abc-123/repo".to_owned(),
                    status: SessionStatusValue::Active,
                },
                SessionInfo {
                    uuid: "def-456".to_owned(),
                    shortname: "session-b".to_owned(),
                    tmux_name: "mux-session-b".to_owned(),
                    workdir: "/home/user/.mux/def-456/repo".to_owned(),
                    status: SessionStatusValue::Dead,
                },
            ],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: ListSessionsResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn kill_session_response_roundtrip() {
        let resp = KillSessionResponse {
            tmux_killed: true,
            workdir_removed: false,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: KillSessionResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn rpc_error_constructor_not_owned() {
        let err = RpcError::not_owned("session not in ownership map");
        assert_eq!(err.error, "not_owned");
        assert_eq!(err.message, "session not in ownership map");
    }

    #[test]
    fn rpc_error_constructor_internal() {
        let err = RpcError::internal("something went wrong");
        assert_eq!(err.error, "internal");
        assert_eq!(err.message, "something went wrong");
    }

    #[test]
    fn request_envelope_health_has_op_tag() {
        let req = Request::Health(HealthRequest {});
        let json = serde_json::to_string(&req).unwrap();
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(val["op"], "Health");
    }

    #[test]
    fn request_envelope_get_session_has_op_and_fields() {
        let req = Request::GetSession(GetSessionRequest {
            uuid: "test-uuid".to_owned(),
        });
        let json = serde_json::to_string(&req).unwrap();
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(val["op"], "GetSession");
        assert_eq!(val["uuid"], "test-uuid");
    }

    #[test]
    fn rpc_result_ok_roundtrip() {
        let result: RpcResult<HealthResponse> = RpcResult::Ok(HealthResponse { ok: true });
        let json = serde_json::to_string(&result).unwrap();
        let decoded: RpcResult<HealthResponse> = serde_json::from_str(&json).unwrap();
        assert_eq!(result, decoded);
        assert!(decoded.into_result().is_ok());
    }

    #[test]
    fn rpc_result_err_roundtrip() {
        let result: RpcResult<HealthResponse> =
            RpcResult::Err(RpcError::not_found("session not found"));
        let json = serde_json::to_string(&result).unwrap();
        let decoded: RpcResult<HealthResponse> = serde_json::from_str(&json).unwrap();
        assert_eq!(result, decoded);
        assert!(decoded.into_result().is_err());
    }

    #[test]
    fn session_status_value_serialises_snake_case() {
        let cases = [
            (SessionStatusValue::Active, "\"active\""),
            (SessionStatusValue::Dead, "\"dead\""),
            (SessionStatusValue::Unreachable, "\"unreachable\""),
            (SessionStatusValue::Orphaned, "\"orphaned\""),
        ];
        for (status, expected) in cases {
            let json = serde_json::to_string(&status).unwrap();
            assert_eq!(
                json, expected,
                "status {:?} should serialise as {}",
                status, expected
            );
        }
    }

    #[test]
    fn shutdown_request_serialises_to_op_tag_only() {
        let req = Request::Shutdown(ShutdownRequest {});
        let json = serde_json::to_string(&req).unwrap();
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(val["op"], "Shutdown");
        // Only the "op" key should be present
        assert_eq!(
            val.as_object().unwrap().len(),
            1,
            "shutdown request should only have the op tag"
        );
    }
}

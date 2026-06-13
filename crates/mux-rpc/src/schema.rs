// RPC request/response types — typed schema implementation in mux-4yg
use serde::{Deserialize, Serialize};

/// Braced-struct form is intentional: serializes to `{}` (JSON object),
/// not `null`, keeping the wire format forward-compatible with future fields.
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthRequest {}

#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub ok: bool,
}

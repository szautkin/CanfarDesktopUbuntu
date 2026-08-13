//! JSON-RPC 2.0 over `serde_json::Value` — the wire layer for the MCP server.
//!
//! Ported from `Mcp/Wire/JsonRpc.cs`. Notifications (absent `id`) get no reply;
//! parse/shape errors become `JsonRpcResponse::failure`. Includes the MCP-specific
//! error codes (`ServerNotInitialized`, `SessionNotApproved`).

use serde_json::{json, Value};

/// JSON-RPC error codes, including the two MCP extensions.
pub mod error_code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    /// MCP: a `tools/*` request arrived before `initialize`.
    pub const SERVER_NOT_INITIALIZED: i64 = -32002;
    /// MCP: the connecting client was not approved by the user.
    pub const SESSION_NOT_APPROVED: i64 = -32001;
}

/// A JSON-RPC request id: a number, a string, or null (kept for error replies).
#[derive(Debug, Clone, PartialEq)]
pub enum JsonRpcId {
    Num(i64),
    Str(String),
    Null,
}

impl JsonRpcId {
    pub fn to_value(&self) -> Value {
        match self {
            JsonRpcId::Num(n) => json!(n),
            JsonRpcId::Str(s) => json!(s),
            JsonRpcId::Null => Value::Null,
        }
    }

    fn from_value(v: &Value) -> JsonRpcId {
        match v {
            Value::Number(n) => JsonRpcId::Num(n.as_i64().unwrap_or(0)),
            Value::String(s) => JsonRpcId::Str(s.clone()),
            _ => JsonRpcId::Null,
        }
    }
}

/// A parsed JSON-RPC request.
#[derive(Debug, Clone)]
pub struct JsonRpcRequest {
    /// `None` for a notification (no reply expected).
    pub id: Option<JsonRpcId>,
    pub method: String,
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }

    /// Parse a request document. Errors carry a human message for `InvalidRequest`.
    pub fn parse(root: &Value) -> Result<JsonRpcRequest, String> {
        let obj = root.as_object().ok_or("request must be a JSON object")?;
        // jsonrpc version is tolerated (2.0 expected) but not strictly enforced.
        let method = obj
            .get("method")
            .and_then(|m| m.as_str())
            .ok_or("missing or non-string \"method\"")?
            .to_string();
        let id = obj.get("id").map(JsonRpcId::from_value);
        let params = obj.get("params").cloned();
        Ok(JsonRpcRequest { id, method, params })
    }
}

/// Build a success response document.
pub fn success(id: &Option<JsonRpcId>, result: Value) -> Value {
    let id_val = id.as_ref().map(|i| i.to_value()).unwrap_or(Value::Null);
    json!({ "jsonrpc": "2.0", "id": id_val, "result": result })
}

/// Build an error response document.
pub fn failure(id: &Option<JsonRpcId>, code: i64, message: &str) -> Value {
    let id_val = id.as_ref().map(|i| i.to_value()).unwrap_or(Value::Null);
    json!({ "jsonrpc": "2.0", "id": id_val, "error": { "code": code, "message": message } })
}

/// A parse-error response with a null id (used before an id can be recovered).
pub fn parse_error(message: &str) -> Value {
    failure(&Some(JsonRpcId::Null), error_code::PARSE_ERROR, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_request_and_notification() {
        let req = JsonRpcRequest::parse(
            &json!({"jsonrpc":"2.0","id":7,"method":"tools/list","params":{}}),
        )
        .unwrap();
        assert_eq!(req.method, "tools/list");
        assert_eq!(req.id, Some(JsonRpcId::Num(7)));
        assert!(!req.is_notification());

        let note =
            JsonRpcRequest::parse(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
                .unwrap();
        assert!(note.is_notification());
    }

    #[test]
    fn missing_method_is_error() {
        assert!(JsonRpcRequest::parse(&json!({"id":1})).is_err());
    }

    #[test]
    fn success_and_failure_shapes() {
        let s = success(&Some(JsonRpcId::Num(1)), json!({"ok":true}));
        assert_eq!(s["id"], json!(1));
        assert_eq!(s["result"]["ok"], json!(true));
        let f = failure(
            &Some(JsonRpcId::Str("x".into())),
            error_code::METHOD_NOT_FOUND,
            "nope",
        );
        assert_eq!(f["error"]["code"], json!(-32601));
        assert_eq!(f["id"], json!("x"));
    }
}

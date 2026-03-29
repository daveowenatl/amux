use serde::{Deserialize, Serialize};

/// JSON-RPC v2 request (newline-delimited).
#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC v2 response (newline-delimited).
#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// Structured error in a JSON-RPC response.
#[derive(Debug, Serialize, Deserialize)]
pub struct RpcError {
    pub code: String,
    pub message: String,
}

/// A server-pushed event (no `id` field — distinguished from responses by `event` field).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerEvent {
    pub event: String,
    pub data: serde_json::Value,
}

impl Response {
    pub fn ok(id: String, result: serde_json::Value) -> Self {
        Self {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: String, code: &str, message: &str) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(RpcError {
                code: code.to_string(),
                message: message.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_ok_response() {
        let resp = Response::ok("1".into(), serde_json::json!({"pong": true}));
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"ok\":true"));
        assert!(json.contains("\"pong\":true"));
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn serialize_err_response() {
        let resp = Response::err("2".into(), "not_found", "no such surface");
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"ok\":false"));
        assert!(json.contains("\"not_found\""));
        assert!(!json.contains("\"result\""));
    }

    #[test]
    fn deserialize_request() {
        let json = r#"{"id":"1","method":"system.ping","params":{}}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "system.ping");
    }

    #[test]
    fn deserialize_request_missing_params() {
        let json = r#"{"id":"1","method":"system.ping"}"#;
        let req: Request = serde_json::from_str(json).unwrap();
        assert_eq!(req.params, serde_json::Value::Null);
    }

    #[test]
    fn roundtrip_response() {
        let resp = Response::ok("42".into(), serde_json::json!({"methods": ["a", "b"]}));
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: Response = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "42");
        assert!(parsed.ok);
    }
}

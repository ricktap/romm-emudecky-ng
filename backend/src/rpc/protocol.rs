//! Wire format of the backend protocol (`docs/architecture/backend-protocol.md`):
//! one strict-JSON object per `\n`-terminated UTF-8 line.

use serde_json::{Value, json};

pub const LIST_METHODS: &str = "__list_methods__";
pub const SHUTDOWN: &str = "__shutdown__";

/// Largest accepted frame (base64 artwork travels as strings).
pub const MAX_LINE_BYTES: usize = 64 * 1024 * 1024;

/// Most events held while no shim is connected.
pub const MAX_BUFFERED_EVENTS: usize = 1000;

/// Serialize one message as a `\n`-terminated line.
pub fn encode(message: &Value) -> String {
    let mut line = message.to_string();
    line.push('\n');
    line
}

pub fn result_response(id: &Value, result: Value) -> Value {
    json!({"id": id, "result": result})
}

pub fn error_response(id: &Value, message: &str) -> Value {
    json!({"id": id, "error": {"message": message}})
}

pub fn event(name: &str, args: Vec<Value>) -> Value {
    json!({"event": name, "args": args})
}

/// A request frame that decoded into a JSON object.
pub struct Request {
    pub id: Value,
    pub method: String,
    pub args: Vec<Value>,
}

/// Why a line is not a valid request; `id` is `Null` when the line was not an object.
pub struct BadFrame {
    pub id: Value,
    pub message: String,
}

/// Decode one line into a request.
pub fn decode_request(line: &[u8]) -> Result<Request, BadFrame> {
    let value: Value = serde_json::from_slice(line).map_err(|e| BadFrame {
        id: Value::Null,
        message: format!("malformed frame: {e}"),
    })?;
    let Value::Object(mut map) = value else {
        return Err(BadFrame {
            id: Value::Null,
            message: "frame is not a JSON object".into(),
        });
    };
    let id = map.remove("id").unwrap_or(Value::Null);
    let args = map.remove("args").unwrap_or_else(|| json!([]));
    match (map.remove("method"), args) {
        (Some(Value::String(method)), Value::Array(args)) => Ok(Request { id, method, args }),
        _ => Err(BadFrame {
            id,
            message: "request needs a string 'method' and list 'args'".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_is_one_line() {
        let line = encode(&json!({"id": 1, "result": "a\nb"}));
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1);
    }

    #[test]
    fn decode_shapes() {
        let r = decode_request(br#"{"id": 3, "method": "get_settings", "args": [1, "x"]}"#)
            .ok()
            .unwrap();
        assert_eq!(
            (r.id, r.method.as_str(), r.args),
            (json!(3), "get_settings", vec![json!(1), json!("x")])
        );
        let r = decode_request(br#"{"id": 4, "method": "__list_methods__"}"#)
            .ok()
            .unwrap();
        assert!(r.args.is_empty());
        let bad = decode_request(b"not json").err().unwrap();
        assert_eq!(bad.id, Value::Null);
        assert!(bad.message.starts_with("malformed frame"));
        let bad = decode_request(b"[1]").err().unwrap();
        assert_eq!(bad.message, "frame is not a JSON object");
        let bad = decode_request(br#"{"id": 9, "method": 5}"#).err().unwrap();
        assert_eq!(bad.id, json!(9));
        let bad = decode_request(br#"{"id": 9, "method": "x", "args": {}}"#)
            .err()
            .unwrap();
        assert_eq!(bad.message, "request needs a string 'method' and list 'args'");
        assert!(
            decode_request(b"{\"id\":1,\"method\":\"x\",\"args\":[NaN]}").is_err(),
            "strict JSON"
        );
    }
}

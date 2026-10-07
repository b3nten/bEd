//! JSON-RPC validation translated from the pinned lsp-framework implementation.
//! The transport keeps arbitrary document bytes outside JSON and requires UTF-8 here.
use serde::de;

use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value};
use std::fmt;

pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;
pub const REQUEST_CANCELLED: i32 = -32800;
pub const CONNECTION_CLOSED: i32 = -32001;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RpcId {
    Null,
    Number(i32),
    String(String),
}

impl RpcId {
    fn parse(value: &Value) -> Result<Self, ResponseError> {
        match value {
            Value::Null => Ok(Self::Null),
            Value::String(value) => Ok(Self::String(value.clone())),
            Value::Number(value) => Ok(Self::Number(integer(value)?)),
            _ => Err(ResponseError::invalid("Invalid request identifier")),
        }
    }

    fn value(&self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Number(value) => Value::from(*value),
            Self::String(value) => Value::from(value.clone()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResponseError {
    pub code: i32,
    pub message: String,
    pub data: Option<Value>,
}

impl ResponseError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(INVALID_REQUEST, message)
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, message)
    }
}

impl fmt::Display for ResponseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for ResponseError {}

#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub id: Option<RpcId>,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub id: RpcId,
    pub result: Result<Value, ResponseError>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Request(Request),
    Response(Response),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Packet {
    Single(Message),
    Batch(Vec<Message>),
}

impl Message {
    fn parse(value: Value) -> Result<Self, ResponseError> {
        let object = value
            .as_object()
            .ok_or_else(|| ResponseError::invalid("Expected JSON-RPC object"))?;
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Err(ResponseError::invalid("Expected jsonrpc 2.0"));
        }
        if let Some(method) = object.get("method") {
            let method = method
                .as_str()
                .ok_or_else(|| ResponseError::invalid("Expected string method"))?;
            let params = match object.get("params") {
                None | Some(Value::Null) => None,
                Some(value @ (Value::Array(_) | Value::Object(_))) => Some(value.clone()),
                _ => {
                    return Err(ResponseError::invalid(
                        "Expected object or array parameters",
                    ));
                }
            };
            return Ok(Self::Request(Request {
                id: object.get("id").map(RpcId::parse).transpose()?,
                method: method.into(),
                params,
            }));
        }
        let id = object
            .get("id")
            .map(RpcId::parse)
            .transpose()?
            .unwrap_or(RpcId::Null);
        let result = match (object.get("result"), object.get("error")) {
            (Some(result), None) => Ok(result.clone()),
            (None, Some(error)) => {
                let error = error
                    .as_object()
                    .ok_or_else(|| ResponseError::invalid("Expected error object"))?;
                let code = error
                    .get("code")
                    .and_then(Value::as_number)
                    .ok_or_else(|| ResponseError::invalid("Expected numeric error code"))?;
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ResponseError::invalid("Expected error message"))?;
                Err(ResponseError {
                    code: integer(code)?,
                    message: message.into(),
                    data: error.get("data").cloned(),
                })
            }
            _ => {
                return Err(ResponseError::invalid(
                    "Expected exactly one result or error",
                ));
            }
        };
        Ok(Self::Response(Response { id, result }))
    }

    pub fn value(&self) -> Value {
        let mut object = Map::new();
        object.insert("jsonrpc".into(), Value::from("2.0"));
        match self {
            Self::Request(request) => {
                if let Some(id) = &request.id {
                    object.insert("id".into(), id.value());
                }
                object.insert("method".into(), Value::from(request.method.clone()));
                if let Some(params) = &request.params {
                    object.insert("params".into(), params.clone());
                }
            }
            Self::Response(response) => {
                object.insert("id".into(), response.id.value());
                match &response.result {
                    Ok(result) => {
                        object.insert("result".into(), result.clone());
                    }
                    Err(error) => {
                        let mut value = Map::new();
                        value.insert("code".into(), Value::from(error.code));
                        value.insert("message".into(), Value::from(error.message.clone()));
                        if let Some(data) = &error.data {
                            value.insert("data".into(), data.clone());
                        }
                        object.insert("error".into(), Value::Object(value));
                    }
                }
            }
        }
        Value::Object(object)
    }
}

impl Packet {
    pub fn parse(bytes: &[u8]) -> Result<Self, ResponseError> {
        let mut deserializer = serde_json::Deserializer::from_slice(bytes);
        let value = UniqueValue::deserialize(&mut deserializer)
            .and_then(|value| {
                deserializer.end()?;
                Ok(value.0)
            })
            .map_err(|error| ResponseError::new(PARSE_ERROR, error.to_string()))?;
        match value {
            Value::Object(_) => Ok(Self::Single(Message::parse(value)?)),
            Value::Array(values) if !values.is_empty() => Ok(Self::Batch(
                values
                    .into_iter()
                    .map(Message::parse)
                    .collect::<Result<_, _>>()?,
            )),
            _ => Err(ResponseError::invalid("Expected object or nonempty batch")),
        }
    }

    pub fn value(&self) -> Value {
        match self {
            Self::Single(message) => message.value(),
            Self::Batch(messages) => Value::Array(messages.iter().map(Message::value).collect()),
        }
    }
}

// The C++ implementation truncates valid decimal IDs to i32. Bound the conversion
// rather than retaining its undefined behavior for out-of-range numbers.
pub(crate) fn integer(value: &Number) -> Result<i32, ResponseError> {
    let number = value
        .as_f64()
        .ok_or_else(|| ResponseError::invalid("Invalid number"))?;
    let truncated = number.trunc();
    if !truncated.is_finite() || truncated < i32::MIN as f64 || truncated > i32::MAX as f64 {
        return Err(ResponseError::invalid("Number outside signed 32-bit range"));
    }
    Ok(truncated as i32)
}

struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueValue;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("JSON value with unique object keys")
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Bool(value)))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::from(value)))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::from(value)))
            }
            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
                Number::from_f64(value)
                    .map(|number| UniqueValue(Value::Number(number)))
                    .ok_or_else(|| E::custom("Nonfinite JSON number"))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::from(value)))
            }
            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::from(value)))
            }
            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element::<UniqueValue>()? {
                    values.push(value.0);
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(de::Error::custom("Duplicate JSON object key"));
                    }
                    values.insert(key, map.next_value::<UniqueValue>()?.0);
                }
                Ok(UniqueValue(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(UniqueVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn validates_batches_identifiers_and_response_shape() {
        assert!(Packet::parse(br#"[]"#).is_err());
        assert!(Packet::parse(br#"[{"jsonrpc":"2.0","method":"a"},false]"#).is_err());
        assert!(Packet::parse(br#"{"jsonrpc":"2.0","result":null,"error":null}"#).is_err());
        assert!(Packet::parse(br#"{"jsonrpc":"2.0","id":2147483648,"result":null}"#).is_err());
        assert!(matches!(
            Packet::parse(br#"{"jsonrpc":"2.0","id":1.9,"result":null}"#).unwrap(),
            Packet::Single(Message::Response(Response {
                id: RpcId::Number(1),
                ..
            }))
        ));
        assert!(matches!(
            Packet::parse(br#"{"jsonrpc":"2.0","method":"x","params":null}"#).unwrap(),
            Packet::Single(Message::Request(Request { params: None, .. }))
        ));
    }
    #[test]
    fn preserves_duplicate_key_rejection_and_strict_utf8() {
        for bytes in [
            br#"{"jsonrpc":"2.0","method":"x","params":{"a":1,"a":2}}"#.as_slice(),
            b"{\"jsonrpc\":\"2.0\",\"method\":\"\xff\"}",
            br#"{"jsonrpc":"2.0","method":"x"} true"#.as_slice(),
        ] {
            assert_eq!(Packet::parse(bytes).unwrap_err().code, PARSE_ERROR);
        }
    }
    #[test]
    fn error_data_is_inside_error() {
        let error = ResponseError {
            code: 12,
            message: "failed".into(),
            data: Some(json!({"detail":42})),
        };
        let packet = Packet::Single(Message::Response(Response {
            id: RpcId::String("id".into()),
            result: Err(error),
        }));
        assert_eq!(packet.value()["error"]["data"]["detail"], 42);
        assert_eq!(
            Packet::parse(&serde_json::to_vec(&packet.value()).unwrap()).unwrap(),
            packet
        );
    }
}

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum SpecialValue {
    Null,
    Undefined,
    NaN,
    Infinity,
    NegInfinity,
    NegZero,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SerializedValue {
    Bool(bool),
    Number(f64),
    Str(String),
    Special(SpecialValue),
    Array(Vec<SerializedValue>),
    Object(Vec<(String, SerializedValue)>),
    Other(String),
}

impl Default for SerializedValue {
    fn default() -> Self {
        SerializedValue::Special(SpecialValue::Undefined)
    }
}

#[derive(Debug, Clone, Default)]
pub struct SerializedArgument {
    pub value: SerializedValue,
}

impl SerializedValue {
    pub fn from_json(value: &Value) -> Self {
        match value {
            Value::Null => SerializedValue::Special(SpecialValue::Null),
            Value::Bool(b) => SerializedValue::Bool(*b),
            Value::Number(n) => SerializedValue::Number(n.as_f64().unwrap_or(0.0)),
            Value::String(s) => SerializedValue::Str(s.clone()),
            Value::Array(items) => {
                SerializedValue::Array(items.iter().map(SerializedValue::from_json).collect())
            }
            Value::Object(map) => SerializedValue::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), SerializedValue::from_json(v)))
                    .collect(),
            ),
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            SerializedValue::Str(s) => Some(s),
            _ => None,
        }
    }
}

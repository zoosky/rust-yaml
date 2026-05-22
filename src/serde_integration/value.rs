//! `Serialize` / `Deserialize` impls for the public `Value` type.

use crate::Value;
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        match self {
            Value::Null => ser.serialize_unit(),
            Value::Bool(b) => ser.serialize_bool(*b),
            Value::Int(i) => ser.serialize_i64(*i),
            Value::Float(f) => ser.serialize_f64(*f),
            Value::String(s) => ser.serialize_str(s),
            Value::Sequence(seq) => {
                let mut s = ser.serialize_seq(Some(seq.len()))?;
                for item in seq {
                    s.serialize_element(item)?;
                }
                s.end()
            }
            Value::Mapping(map) => {
                let mut m = ser.serialize_map(Some(map.len()))?;
                for (k, v) in map {
                    m.serialize_entry(k, v)?;
                }
                m.end()
            }
        }
    }
}

#[cfg(test)]
mod ser_tests {
    use crate::Value;
    use indexmap::IndexMap;

    #[test]
    fn value_serializes_through_serde_json() {
        let mut m = IndexMap::new();
        m.insert(Value::String("name".into()), Value::String("rust".into()));
        m.insert(Value::String("ver".into()), Value::Int(11));
        let v = Value::Mapping(m);

        let j = serde_json::to_string(&v).expect("serialize through serde_json");
        assert_eq!(j, r#"{"name":"rust","ver":11}"#);
    }

    #[test]
    fn null_int_float_bool_seq_serialize() {
        assert_eq!(serde_json::to_string(&Value::Null).unwrap(), "null");
        assert_eq!(serde_json::to_string(&Value::Bool(true)).unwrap(), "true");
        assert_eq!(serde_json::to_string(&Value::Int(42)).unwrap(), "42");
        assert_eq!(serde_json::to_string(&Value::Float(1.5)).unwrap(), "1.5");
        let seq = Value::Sequence(vec![Value::Int(1), Value::Int(2)]);
        assert_eq!(serde_json::to_string(&seq).unwrap(), "[1,2]");
    }
}

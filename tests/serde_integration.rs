#![cfg(feature = "serde")]

use indexmap::IndexMap;
use proptest::prelude::*;
use rust_yaml::Value;

fn arb_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::Int),
        any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(Value::Float),
        // Restrict strings to printable ASCII to dodge YAML quoting quirks
        // unrelated to serde correctness.
        "[a-zA-Z0-9_]{0,16}".prop_map(Value::String),
    ];
    leaf.prop_recursive(3, 32, 8, |inner: BoxedStrategy<Value>| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Sequence),
            prop::collection::vec(("[a-z]{1,8}", inner), 0..6).prop_map(|kvs| {
                let mut m = IndexMap::new();
                for (k, v) in kvs {
                    m.insert(Value::String(k), v);
                }
                Value::Mapping(m)
            }),
        ]
    })
}

proptest! {
    #[test]
    fn value_round_trips_through_serde(v in arb_value()) {
        let yaml = rust_yaml::to_string(&v).expect("dump");
        let back: Value = rust_yaml::from_str(&yaml).expect("load");
        prop_assert_eq!(v, back);
    }
}

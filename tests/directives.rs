//! Tests for YAML directives (%YAML and %TAG)

use rust_yaml::{Value, Yaml};

#[test]
fn test_yaml_version_directive() {
    let yaml_input = r#"%YAML 1.2
---
foo: bar
"#;

    let yaml = Yaml::new();
    let result = yaml.load_str(yaml_input).unwrap();

    // Should parse the document content correctly
    if let Value::Mapping(map) = result {
        assert_eq!(
            map.get(&Value::String("foo".to_string())),
            Some(&Value::String("bar".to_string()))
        );
    } else {
        panic!("Expected mapping");
    }
}

#[test]
fn test_tag_directive_basic() {
    let yaml_input = r#"%TAG ! tag:example.com,2024:
---
!person
name: John Doe
age: 30
"#;

    let yaml = Yaml::new();
    let result = yaml.load_str(yaml_input);

    // Should handle tag directives (even if not fully resolved yet)
    assert!(result.is_ok(), "Should parse document with tag directive");
}

#[test]
fn test_multiple_tag_directives() {
    let yaml_input = r#"%TAG ! tag:example.com,2024:
%TAG !! tag:yaml.org,2002:
---
!!str "Hello"
"#;

    let yaml = Yaml::new();
    let result = yaml.load_str(yaml_input);

    assert!(
        result.is_ok(),
        "Should parse document with multiple tag directives"
    );
}

#[test]
fn test_directives_with_multiple_documents() {
    let yaml_input = r#"%YAML 1.2
---
doc1: value1
...
%YAML 1.2
---
doc2: value2
"#;

    let yaml = Yaml::new();

    // Load all documents
    let documents = yaml.load_all_str(yaml_input).unwrap();

    assert_eq!(documents.len(), 2, "Should parse two documents");

    // Check first document
    if let Value::Mapping(map) = &documents[0] {
        assert_eq!(
            map.get(&Value::String("doc1".to_string())),
            Some(&Value::String("value1".to_string()))
        );
    } else {
        panic!("Expected mapping for first document");
    }

    // Check second document
    if let Value::Mapping(map) = &documents[1] {
        assert_eq!(
            map.get(&Value::String("doc2".to_string())),
            Some(&Value::String("value2".to_string()))
        );
    } else {
        panic!("Expected mapping for second document");
    }
}

#[test]
fn test_implicit_document_with_directives() {
    // Document without explicit --- should still work with directives
    let yaml_input = r#"%YAML 1.2
%TAG ! tag:example.com,2024:
key: value
"#;

    let yaml = Yaml::new();
    let result = yaml.load_str(yaml_input).unwrap();

    if let Value::Mapping(map) = result {
        assert_eq!(
            map.get(&Value::String("key".to_string())),
            Some(&Value::String("value".to_string()))
        );
    } else {
        panic!("Expected mapping");
    }
}

#[test]
fn test_tag_directive_with_handle() {
    let yaml_input = r#"%TAG !ex! tag:example.com,2024:
---
!ex!widget
id: 123
type: button
"#;

    let yaml = Yaml::new();
    let result = yaml.load_str(yaml_input);

    assert!(
        result.is_ok(),
        "Should parse document with named tag handle"
    );
}

#[test]
fn test_directives_only_apply_to_next_document() {
    let yaml_input = r#"%YAML 1.2
%TAG ! tag:example.com,2024:
---
doc1: with_directives
...
---
doc2: without_directives
"#;

    let yaml = Yaml::new();
    let documents = yaml.load_all_str(yaml_input).unwrap();

    assert_eq!(documents.len(), 2, "Should parse both documents");

    // Both documents should parse correctly
    // The directives only apply to the first document
    for (i, doc) in documents.iter().enumerate() {
        if let Value::Mapping(_) = doc {
            // Good - parsed as mapping
        } else {
            panic!("Document {} should be a mapping", i + 1);
        }
    }
}

#[test]
fn test_yaml_version_1_1_compatibility() {
    // Under %YAML 1.1, `yes`/`no` are booleans, not strings.
    let yaml_input = r#"%YAML 1.1
---
yes: true
no: false
"#;

    let yaml = Yaml::new();
    let result = yaml.load_str(yaml_input).expect("parse YAML 1.1 document");

    let Value::Mapping(map) = result else {
        panic!("expected mapping, got {result:?}");
    };

    // Both keys and values are booleans under 1.1.
    let yes_val = map
        .get(&Value::Bool(true))
        .expect("`yes` key as Bool(true)");
    assert_eq!(yes_val, &Value::Bool(true));
    let no_val = map
        .get(&Value::Bool(false))
        .expect("`no` key as Bool(false)");
    assert_eq!(no_val, &Value::Bool(false));
    assert_eq!(map.len(), 2, "exactly two entries");
}

#[test]
fn test_yaml_1_2_default_treats_legacy_bools_as_strings() {
    // No %YAML directive => 1.2 semantics. yes/no are plain strings.
    let yaml_input = "yes: hello\nno: world\n";

    let yaml = Yaml::new();
    let Value::Mapping(map) = yaml.load_str(yaml_input).expect("parse") else {
        panic!("expected mapping");
    };

    assert_eq!(
        map.get(&Value::String("yes".to_string())),
        Some(&Value::String("hello".to_string())),
        "`yes` is a string key under 1.2 default"
    );
    assert_eq!(
        map.get(&Value::String("no".to_string())),
        Some(&Value::String("world".to_string())),
        "`no` is a string key under 1.2 default"
    );
    assert!(
        map.get(&Value::Bool(true)).is_none(),
        "no Bool(true) key should be created"
    );
}

#[test]
fn test_yaml_1_2_explicit_directive_keeps_1_2_semantics() {
    // %YAML 1.2 is the spec default — yes/no should remain strings.
    let yaml_input = "%YAML 1.2\n---\nyes: hi\n";

    let yaml = Yaml::new();
    let Value::Mapping(map) = yaml.load_str(yaml_input).expect("parse") else {
        panic!("expected mapping");
    };
    assert_eq!(
        map.get(&Value::String("yes".to_string())),
        Some(&Value::String("hi".to_string())),
        "%YAML 1.2 keeps `yes` as string"
    );
}

#[test]
fn test_yaml_version_directive_does_not_carry_across_documents() {
    // Per YAML 1.2.2 spec §9.1.3, directives apply only to the document that
    // follows them. A second document without a directive must use the
    // default version (1.2).
    let yaml_input = r#"%YAML 1.1
---
flag: yes
---
flag: yes
"#;

    let yaml = Yaml::new();
    let docs = yaml
        .load_all_str(yaml_input)
        .expect("parse multi-doc stream");
    assert_eq!(docs.len(), 2, "two documents in stream");

    // Doc 1: %YAML 1.1 in effect, `yes` is bool.
    let Value::Mapping(doc1) = &docs[0] else {
        panic!("doc1 expected mapping");
    };
    assert_eq!(
        doc1.get(&Value::String("flag".to_string())),
        Some(&Value::Bool(true)),
        "doc1 under 1.1: yes is Bool(true)"
    );

    // Doc 2: no directive => default 1.2, `yes` is string.
    let Value::Mapping(doc2) = &docs[1] else {
        panic!("doc2 expected mapping");
    };
    assert_eq!(
        doc2.get(&Value::String("flag".to_string())),
        Some(&Value::String("yes".to_string())),
        "doc2 reverts to 1.2 default: yes is String"
    );
}

#[test]
fn test_yaml_1_1_value_tag_token_still_string_for_now() {
    // The `=` value-key replacement token is a YAML 1.1 feature (`!!value`
    // tag) that was dropped in 1.2. Full 1.1 construction of `=` is
    // deferred — for now we just document the current behavior so a future
    // implementer notices when this changes.
    let yaml_input = "%YAML 1.1\n---\nitems:\n  - =\n";
    let yaml = Yaml::new();
    let result = yaml.load_str(yaml_input).expect("parse");
    let Value::Mapping(map) = result else {
        panic!("expected mapping");
    };
    let items = map.get(&Value::String("items".to_string())).expect("items");
    assert_eq!(
        items,
        &Value::Sequence(vec![Value::String("=".to_string())]),
        "`=` under 1.1 currently parses as String — full !!value handling tracked separately"
    );
}

#[test]
fn test_directive_scanner_integration() {
    // Test that directives are properly scanned and passed through the pipeline
    let yaml_input = r#"%YAML 1.2
%TAG !foo! tag:example.com,2024/foo:
%TAG !bar! tag:example.com,2024/bar:
---
regular: value
!foo!widget: component
!bar!config: settings
"#;

    let yaml = Yaml::new();
    let result = yaml.load_str(yaml_input);

    // Should not error even with complex directives
    assert!(
        result.is_ok(),
        "Should handle multiple custom tag directives"
    );
}

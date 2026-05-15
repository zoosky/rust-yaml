//! YAML test suite integration for rust-yaml.
//!
//! This crate is dev-only and not published. It consumes the official
//! [yaml/yaml-test-suite](https://github.com/yaml/yaml-test-suite) `data/`
//! directory format and exposes helpers to drive each test case against
//! `rust-yaml`'s parser, comparing emitted events against the canonical
//! `test.event` trees.

use std::path::Path;

use rust_yaml::parser::{BasicParser, EventType, Parser as ParserTrait, ScalarStyle};

// ─── Data-directory test case ────────────────────────────────────────

/// A test case from the `data/` directory format.
#[derive(Debug)]
pub struct DataTestCase {
    /// Test ID (e.g. `"229Q"` or `"2G84/00"`).
    pub id: String,
    /// Human-readable name from the `===` file.
    pub name: String,
    /// Raw YAML input from `in.yaml`.
    pub yaml: String,
    /// Expected event lines from `test.event`.
    pub expected_events: Vec<String>,
    /// Whether `error` file exists (parser must reject the input).
    pub expect_error: bool,
}

/// Load a single test case from a directory containing `in.yaml`, `test.event`, etc.
fn load_single_test(dir: &Path, id: String) -> Option<DataTestCase> {
    let in_yaml = dir.join("in.yaml");
    if !in_yaml.exists() {
        return None;
    }

    let yaml = std::fs::read_to_string(&in_yaml)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", in_yaml.display()));

    let name_path = dir.join("===");
    let name = if name_path.exists() {
        std::fs::read_to_string(name_path)
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        String::new()
    };

    let event_path = dir.join("test.event");
    let expected_events = if event_path.exists() {
        let content = std::fs::read_to_string(event_path).unwrap_or_default();
        content
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    } else {
        Vec::new()
    };

    let expect_error = dir.join("error").exists();

    Some(DataTestCase {
        id,
        name,
        yaml,
        expected_events,
        expect_error,
    })
}

/// Load all tests from the `data/` directory.
///
/// Discovers 4-char alphanumeric test directories. Handles both single-test
/// directories (containing `in.yaml`) and multi-subtest directories
/// (containing numbered subdirectories `00/`, `01/`, …).
pub fn load_all_tests(data_dir: &Path) -> Vec<DataTestCase> {
    let mut entries: Vec<_> = std::fs::read_dir(data_dir)
        .expect("failed to read data directory")
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            // 4-char alphanumeric test IDs only (skips `name/`, `tags/`).
            name.len() == 4 && name.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);

    let mut tests = Vec::new();

    for entry in entries {
        let test_id = entry.file_name().to_string_lossy().to_string();
        let dir = entry.path();

        if dir.join("in.yaml").exists() {
            if let Some(tc) = load_single_test(&dir, test_id) {
                tests.push(tc);
            }
        } else {
            let mut subdirs: Vec<_> = std::fs::read_dir(&dir)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", dir.display()))
                .filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .collect();
            subdirs.sort_by_key(std::fs::DirEntry::file_name);

            for sub in subdirs {
                let sub_name = sub.file_name().to_string_lossy().to_string();
                let sub_id = format!("{test_id}/{sub_name}");
                if let Some(tc) = load_single_test(&sub.path(), sub_id) {
                    tests.push(tc);
                }
            }
        }
    }

    tests
}

// ─── Event format conversion ────────────────────────────────────────

/// Run the rust-yaml parser on `input` and return events in the test suite
/// tree format. Returns `Err` if the parser produces an error.
pub fn events_to_tree(input: &str) -> Result<Vec<String>, String> {
    let mut parser = BasicParser::new_eager(input.to_string());

    if let Some(err) = parser.take_scanning_error() {
        return Err(err.to_string());
    }

    let mut lines = Vec::new();
    loop {
        match parser.get_event() {
            Ok(Some(event)) => lines.push(event_to_tree_line(&event.event_type)),
            Ok(None) => break,
            Err(e) => return Err(e.to_string()),
        }
    }

    Ok(lines)
}

/// Convert a single event to its tree-format string.
fn event_to_tree_line(kind: &EventType) -> String {
    match kind {
        EventType::StreamStart => "+STR".to_string(),
        EventType::StreamEnd => "-STR".to_string(),
        EventType::DocumentStart { implicit, .. } => {
            if *implicit {
                "+DOC".to_string()
            } else {
                "+DOC ---".to_string()
            }
        }
        EventType::DocumentEnd { implicit } => {
            if *implicit {
                "-DOC".to_string()
            } else {
                "-DOC ...".to_string()
            }
        }
        EventType::MappingStart {
            anchor,
            tag,
            flow_style,
        } => {
            let mut s = "+MAP".to_string();
            if *flow_style {
                s.push_str(" {}");
            }
            if let Some(a) = anchor {
                s.push_str(&format!(" &{a}"));
            }
            if let Some(t) = tag {
                s.push_str(&format!(" <{t}>"));
            }
            s
        }
        EventType::MappingEnd => "-MAP".to_string(),
        EventType::SequenceStart {
            anchor,
            tag,
            flow_style,
        } => {
            let mut s = "+SEQ".to_string();
            if *flow_style {
                s.push_str(" []");
            }
            if let Some(a) = anchor {
                s.push_str(&format!(" &{a}"));
            }
            if let Some(t) = tag {
                s.push_str(&format!(" <{t}>"));
            }
            s
        }
        EventType::SequenceEnd => "-SEQ".to_string(),
        EventType::Scalar {
            value,
            style,
            anchor,
            tag,
            ..
        } => {
            let mut s = "=VAL".to_string();
            if let Some(a) = anchor {
                s.push_str(&format!(" &{a}"));
            }
            if let Some(t) = tag {
                s.push_str(&format!(" <{t}>"));
            }
            let style_char = match style {
                ScalarStyle::Plain => ':',
                ScalarStyle::SingleQuoted => '\'',
                ScalarStyle::DoubleQuoted => '"',
                ScalarStyle::Literal => '|',
                ScalarStyle::Folded => '>',
            };
            s.push_str(&format!(" {style_char}{}", escape_value(value)));
            s
        }
        EventType::Alias { anchor } => format!("=ALI *{anchor}"),
    }
}

/// Escape special characters in scalar values for the tree format.
fn escape_value(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
        .replace('\x08', "\\b")
        .replace('\0', "\\0")
}

// ─── Diff display ───────────────────────────────────────────────────

/// Show the first difference between expected and actual event trees.
pub fn show_diff(expected: &[String], actual: &[String]) -> String {
    let max = expected.len().max(actual.len());
    for i in 0..max {
        let exp = expected.get(i).map(String::as_str).unwrap_or("<missing>");
        let act = actual.get(i).map(String::as_str).unwrap_or("<missing>");
        if exp != act {
            return format!(
                "  line {}: expected: {exp:?}\n  line {}:   actual: {act:?}",
                i + 1,
                i + 1
            );
        }
    }
    "  (no difference found)".to_string()
}

// ─── Test runner ────────────────────────────────────────────────────

/// Result of running a single test case.
#[derive(Debug)]
pub enum TestResult {
    /// Test passed — events match expected tree (or error was correctly raised).
    Pass,
    /// Test failed — events don't match expected tree, or wrong error behavior.
    Fail(String),
}

/// Run a single test case and return the result.
pub fn run_test(tc: &DataTestCase) -> TestResult {
    if tc.expect_error {
        match events_to_tree(&tc.yaml) {
            Err(_) => TestResult::Pass,
            Ok(_) => TestResult::Fail("expected error but parser succeeded".to_string()),
        }
    } else {
        match events_to_tree(&tc.yaml) {
            Ok(actual) => {
                if tc.expected_events.is_empty() {
                    // No test.event reference — fall back to parse-success.
                    TestResult::Pass
                } else if actual == tc.expected_events {
                    TestResult::Pass
                } else {
                    TestResult::Fail(show_diff(&tc.expected_events, &actual))
                }
            }
            Err(e) => TestResult::Fail(format!("unexpected error: {e}")),
        }
    }
}

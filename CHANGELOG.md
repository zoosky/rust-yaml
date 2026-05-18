# Changelog

All notable changes to **rust-yaml** are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and
this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] — 2026-05-18

### Highlights

🎯 **100% conformance with the upstream
[`yaml/yaml-test-suite`](https://github.com/yaml/yaml-test-suite)
(`data-2022-01-17` pin)** — 735 / 735 spec-conformance tests pass. This is the
first rust-yaml release with full YAML 1.2 spec conformance verified by the
official test corpus.

### Added

- **`yaml-test-suite` workspace crate** — a separate, non-published harness
  that drives the upstream conformance corpus, classifies failures
  (wrong-reject / wrong-accept / wrong-events / timeout), and writes a
  per-test failure report to `target/yaml-test-suite-failures.txt`.
- **YAML 1.1 `%YAML` directive honored** for plain-scalar resolution: when
  `%YAML 1.1` is in effect, `yes`/`no`/`on`/`off` resolve as booleans;
  default `1.2` keeps them as strings.
- **Per-document `%TAG` directive scope** (§6.8): directives apply to one
  document only; the resolver resets across `---` boundaries.
- **Named-tag-handle validation**: `!prefix!suffix` without a matching
  `%TAG` directive in scope is now correctly rejected.
- **Streaming + comment-preserving paths** strengthened to surface
  eager-parse errors via `get_event()` (previously silently swallowed).
- **`.prettierrc.json` + `.prettierignore`** to keep markdown/YAML/JSON edits
  consistent across the project (does not touch `*.rs`/`*.toml` — those stay
  under `rustfmt`).
- **Strict `make ci` gate**: `cargo clippy --all-targets --all-features
  -- -D warnings -D clippy::pedantic` with a curated allow-list now blocking;
  every PR must pass `make ci` to merge.

### Changed

- **Empty plain scalar now resolves to `Null`** (§10.2 Core Schema) rather
  than `String("")`. Affects any code that relied on the previous behavior.
- **`%YAML` / `%TAG` directives** no longer leak across documents in a
  multi-doc stream.
- **Tab handling tightened** to spec:
  - Pure-tab indentation (`\tkey: value`) is rejected per §6.1.
  - Tab as the separator after block indicators `?`, `:`, `-` is rejected
    where the spec requires a space.
  - Tab inside multi-line quoted-scalar continuation is rejected when
    serving as indentation in a nested block context.
  - Carve-outs preserved: tab after spaces in value-continuation lines,
    tab before top-level flow openers (`\t[`, `\t{`).
- **Block-scalar header**: comment after the indicator now requires
  preceding whitespace (`|#x` and `>#x` are invalid).
- **Tag suffix** terminates on `,` in both block and flow contexts
  (`,` must be percent-encoded as `%2C` to be part of a tag).

### Fixed

68 parser/scanner fixes drove the suite from 82.4 % (606 / 735) at the start
of the conformance push to 100.0 % (735 / 735). The biggest classes were:

- **Line-start property handling** — anchors, tags, aliases, and flow openers
  (`[`/`{`) at line-start now correctly open the implicit block mapping at
  their own column, so the property attaches to the right node.
- **Freestanding properties** (anchor/tag on a previous line, then a key on
  the next) — flushed as the previous node's empty value instead of leaking
  onto the next node.
- **State-machine completeness** — `?` accepted in `BlockSequence` and
  `BlockMappingValue`, `:` after `BlockEntry` opens a single-pair item
  mapping, missing-value synthesis preserves pending properties.
- **Indent rules** — strict multiple-of-N indent removed (`§6.1`); post-dedent
  indent must match an open container level; flow-content indent enforced
  inside enclosing blocks; deeper indent rejected when parent has a
  complete key-value pair.
- **Implicit-key constraints** — `:` after a multi-line flow collection is
  rejected as an implicit key; multiple `:` on the same line in a block
  mapping is rejected; sequence value on same line as implicit key
  (`key: - a`) is rejected.
- **Flow-collection-as-key** — `[a]: x` and `{x: y}: z` are now wrapped
  in the implicit single-pair mapping the spec requires.
- **Inline single-pair mappings as explicit keys / values** —
  `? earth: blue\n  : moon: white` and `? []: x` produce the nested
  mappings the spec expects.
- **Empty-block-entry synth** — a bare `-` at EOF produces an implicit
  empty scalar item.
- **Nested block sequences across lines** — `- - x\n  - y` no longer
  prematurely closes the inner sequence.
- **Doc markers inside quoted scalars** — `---` or `...` at column 1 inside
  an unterminated quoted string is rejected.
- **Tag terminator in flow context** — `!!str,` in flow context now
  correctly emits the comma as a `FlowEntry` rather than swallowing it
  into the tag.

(Full per-commit list: `git log v0.0.5..v1.0.0`.)

### Removed

- The non-spec "multiple of N indent width" strictness check
  (rejected valid spec fixtures like Spec Example 6.1).

### Compatibility

Per the v0.x policy, breaking changes were permitted in minor releases.
The 1.0.0 release stabilizes the public API; future breaking changes will
follow strict semver (major bump, deprecation period of ≥ 2 minor versions,
migration guide).

The most visible behavioral change for consumers is that **`test:`** (key
with no value) now resolves the value to `Value::Null` instead of
`Value::String("")`. Callers that previously matched on the empty-string
shape should switch to matching `Value::Null` (or use `is_null()`).

---

## [0.0.5] — 2025-08-20

Earlier releases (0.0.1 — 0.0.5) supported a broad working subset of
YAML 1.2 with documented gaps. See the v0.0.5 GitHub release notes for
details.

[1.0.0]: https://github.com/elioetibr/rust-yaml/releases/tag/v1.0.0
[0.0.5]: https://github.com/elioetibr/rust-yaml/releases/tag/v0.0.5

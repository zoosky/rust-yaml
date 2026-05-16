# YAML Test Suite Conformance Roadmap

Live tracker for closing the gap between rust-yaml and the yaml/yaml-test-suite
(2022-01-17 pin) corpus.

## Current state

| Metric          | Value          |
| --------------- | -------------- |
| Tests passing   | **341 / 735** (46.4 %) |
| Parser hangs    | 0 ✅           |
| Wrong-reject    | 42             |
| Wrong-accept    | 114            |
| Wrong-events    | 238            |
| Lib unit tests  | 163 passing    |

Live results are written to `target/yaml-test-suite-failures.txt` after every
`make test-yaml-suite` run. Categories: `Timeouts`, `Wrong reject`,
`Wrong accept`, `Wrong events`.

## Done so far

22 TDD-driven parser/scanner fixes, each with a regression test in
`src/scanner/tests` or `src/parser/tests`. Session 1 commits (250/735 milestone):

* `0768dcf` Stop `---word1` infinite loops (all 7 hangs gone).
* `6b2ea6a` Ignore unknown directives (spec §6.8.4).
* `f1f680d` Tag URI percent-escapes + decoding.
* `08282fb` `?` / `:` / `-` as plain-scalar starts when not indicators.
* `fb01299` Full unicode in anchor / alias names.
* `370c15a` Complex-key marker after explicit `---`.
* `2e7abfb` Always emit `-DOC` before `-STR` when a doc is open (+28).
* `db46fa4` Multi-line plain-scalar folding (§6.5 / §7.3.3, +12).
* `e012676` Double-quoted escape allowlist (§5.7).
* `8949016` `\<tab>` escape support.
* `587e7dd` `\x##` / `\u####` / `\U########` hex escapes.
* `da8c8ff` Reject a second anchor on the same node.
* `a10aaeb` Reject a second tag on the same node.
* `935d7e1` Error on content after `...` document-end marker.
* `798dd02` Reject aliases pointing at undefined anchors.

Session 2 commits (250 → 314 = +64):

* `f1e9050` Quoted-scalar line folding §7.3.2 (+29; biggest single win).
* `d74791a` Single-quote `''` escape + `\<NL>` whitespace strip (+10).
* `660b445` `:` adjacent to value in flow context (+6).
* `9c0398f` Reject `---` / `...` inside flow collections (+2).
* `6e71fcf` Reject trailing content after quoted scalar (+4).
* `cfa7f69` Surface eager-parse errors + reject duplicate %YAML (+2).
* `38d3f19` Reject anchor / tag on alias node (+4).
* `fce1927` Reject directives outside the directive context (+7).

Session 3 commits (314 → 341 = +27):

* `02526b6` Transition to ImplicitDocumentStart after `...` (+2).
* `e4d6409` Reject `#` adjacent to quoted scalar without space (+2).
* `4c1ce52` Stop plain scalar at `:` followed by flow indicator.
* `05f27a8` Error on unclosed quoted strings (+2).
* `2619819` Comment `#` requires preceding whitespace (+4).
* `7de06b7` Reject directive without document body at EOS (+4).
* `60dceb4` Reject extra content after %YAML directive (+3).
* `bfc33b2` Implicit empty scalar for empty `---` document (+6).
* `5f0096d` Implicit empty scalar between back-to-back `---` (+2).
* `59fa0b0` Implicit empty scalar for `---\n...` empty document (+2).
* `0dbee5e` Relax over-strict double-tag check (net 0; wrong-reject -4).

## Blocked clusters (need deeper refactors)

These clusters are blocked because attempted relaxations either trigger
lib-test regressions or net-negative suite movement. Each requires a focused
investigation, ideally as its own session.

### A. Strict-multiple-of-N indentation (10+ wrong-rejects)

Tests: **6HB6, 8G76, A2M4, F6MC, P94K, Q9WF, UGM3** and others.

`BasicScanner::handle_indentation` rejects indents that aren't a multiple of
the first observed width (e.g. forbids `3` once `2` is detected). YAML 1.2
§6.1 has no such rule — siblings must share a column, children must indent
further, but any positive amount works.

Removing the strict check fixed +14 wrong-rejects but exposed +18
wrong-accept and wrong-events latent bugs — net -4 passes. Verified twice.

**Path forward:** Track per-block "minimum-indent" on the indent stack and
enforce "child indent > parent indent" without the multiple-of-N rule. The
fix likely also needs companion validations elsewhere (block-scalar header,
flow-collection bracket-balance) so the latent bugs don't leak through.

### B. Block-scalar `{` / `}` content routing (1 lib regression)

Tests including those in the **wrong-accept "Flow sequence" cluster** (4H7K,
6JTT, etc.) need stricter bracket balancing — but adding `flow_level == 0`
guards in the `]` / `}` arms broke `tests::test_complex_yaml_document`. Why:
a literal `|` block scalar containing `{`/`}` inside its content leaks those
characters to the main dispatcher.

**Path forward:** Make `scan_literal_block_scalar` / `scan_folded_block_scalar`
robustly consume all characters within the scalar's indent boundary so
nothing reaches `process_line`. After that, the bracket guards become safe.

### C. Multi-line scalar parent-aware indent threshold

Tests: **36F6, 4CQQ, FBC9** and many wrong-events.

The current fold logic uses `next_col >= start_col`. Per spec the rule
should be `next_col > parent_key_indent`. Attempted via `last_token == Value`
detection but the parent_indent value from `indent_stack` is the wrong
threshold — broke `tests::test_round_trip_nested_structure` (-34 net).

**Path forward:** Track the "containing block's column" separately when
entering each block, not just the abstract indent value. Pass that
threshold into `scan_plain_scalar`.

### D. Eager-parse error swallowing

`BasicParser::new_eager` calls `parse_all().unwrap_or(())`, hiding all
parser-side validation errors. Adding error surfacing flipped 8 wrong-accepts
to correct rejections but also exposed +7 partial-parse cases that had been
"passing" by accident.

**Path forward:** Surface errors, *then* TDD each newly-exposed partial-parse
case until they're proper failures vs. proper passes.

### E. Complex-key in BlockMappingValue state

Tests: **KK5P** and similar nested-complex-key fixtures.

The Key (`?`) token handler in `BasicParser::process_token` covers
`ImplicitDocumentStart` / `DocumentContent` / `DocumentStart` (added this
push) / `BlockMapping` / `FlowMapping` / `BlockMappingKey` / `FlowMappingKey`
but not `BlockMappingValue`. A `?` inside a value position should open a
nested mapping.

**Path forward:** Add a `BlockMappingValue` arm that pushes the current
state, emits `BlockMappingStart`, and transitions to `BlockMappingKey`.
Needs care so the value state is restored when the inner mapping closes.

### F. Empty-key mappings (`: value` at line head)

Tests: **2JQS** (`: a\n: b`) and similar.

The scanner's `:` arm only emits a Value token; there's no synthesized
empty scalar before it, and no `BlockMappingStart` if not already in a
mapping context.

**Path forward:** When `:` is at column 1 (or generally at a block-mapping
indent) followed by whitespace, emit (a) `BlockMappingStart` if needed,
(b) an empty `Scalar` for the missing key, then (c) the existing `Value`
token.

## Other observed-but-not-yet-attacked issues

* **Quoted scalars at line head not detected as mapping keys** (26DV, 2EBW
  if quoted). Attempted the same `check_for_mapping_ahead` dispatch but
  regressed -2; needs scoping investigation.
* **Anchor-name `:` boundary** (2SXE). Currently includes `:` in anchor;
  spec is ambiguous here.
* **`---` followed by mapping on the same line** (9KBC, CXX2). Per the
  test suite, invalid; per some interpretations of the spec, valid.
* **Directives mid-stream without `...`** (9HCY). Validation written but
  blocked by D above.
* **Block-scalar header validation** (5LLU and others). Header is parsed but
  not validated against malformed indent indicators.
* **Tabs in indentation** (DK95/00, HS5T, 4EJS). Currently sometimes
  rejected, sometimes accepted inconsistently.

## Test discipline

Each fix in this push followed strict TDD:

1. Identify a failing yaml-test-suite fixture.
2. Write a unit test in `src/scanner/tests` or `src/parser/tests` that
   parses the same input and asserts the expected behaviour (RED).
3. Make the smallest possible change to the parser/scanner.
4. Re-run `cargo test --lib` (GREEN).
5. Re-run `make test-yaml-suite` to confirm net-positive movement and no
   lib regressions.
6. Commit GPG-signed (no `Co-Authored-By` per project policy).

Revertable per fix; the harness produces a fresh categorized report on every
run so progress / regressions show up immediately.

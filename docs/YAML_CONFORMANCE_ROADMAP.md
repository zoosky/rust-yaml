# YAML Test Suite Conformance Roadmap

Live tracker for closing the gap between rust-yaml and the yaml/yaml-test-suite
(2022-01-17 pin) corpus.

## Current state

| Metric         | Value                  |
| -------------- | ---------------------- |
| Tests passing  | **687 / 735** (93.5 %) |
| Parser hangs   | 0 ✅                   |
| Wrong-reject   | 3                      |
| Wrong-accept   | 33                     |
| Wrong-events   | 12                     |
| Lib unit tests | 185 passing            |

Live results are written to `target/yaml-test-suite-failures.txt` after every
`make test-yaml-suite` run. Categories: `Timeouts`, `Wrong reject`,
`Wrong accept`, `Wrong events`.

## Done so far

22 TDD-driven parser/scanner fixes, each with a regression test in
`src/scanner/tests` or `src/parser/tests`. Session 1 commits (250/735 milestone):

- `0768dcf` Stop `---word1` infinite loops (all 7 hangs gone).
- `6b2ea6a` Ignore unknown directives (spec §6.8.4).
- `f1f680d` Tag URI percent-escapes + decoding.
- `08282fb` `?` / `:` / `-` as plain-scalar starts when not indicators.
- `fb01299` Full unicode in anchor / alias names.
- `370c15a` Complex-key marker after explicit `---`.
- `2e7abfb` Always emit `-DOC` before `-STR` when a doc is open (+28).
- `db46fa4` Multi-line plain-scalar folding (§6.5 / §7.3.3, +12).
- `e012676` Double-quoted escape allowlist (§5.7).
- `8949016` `\<tab>` escape support.
- `587e7dd` `\x##` / `\u####` / `\U########` hex escapes.
- `da8c8ff` Reject a second anchor on the same node.
- `a10aaeb` Reject a second tag on the same node.
- `935d7e1` Error on content after `...` document-end marker.
- `798dd02` Reject aliases pointing at undefined anchors.

Session 2 commits (250 → 314 = +64):

- `f1e9050` Quoted-scalar line folding §7.3.2 (+29; biggest single win).
- `d74791a` Single-quote `''` escape + `\<NL>` whitespace strip (+10).
- `660b445` `:` adjacent to value in flow context (+6).
- `9c0398f` Reject `---` / `...` inside flow collections (+2).
- `6e71fcf` Reject trailing content after quoted scalar (+4).
- `cfa7f69` Surface eager-parse errors + reject duplicate %YAML (+2).
- `38d3f19` Reject anchor / tag on alias node (+4).
- `fce1927` Reject directives outside the directive context (+7).

Session 3 commits (314 → 341 = +27):

- `02526b6` Transition to ImplicitDocumentStart after `...` (+2).
- `e4d6409` Reject `#` adjacent to quoted scalar without space (+2).
- `4c1ce52` Stop plain scalar at `:` followed by flow indicator.
- `05f27a8` Error on unclosed quoted strings (+2).
- `2619819` Comment `#` requires preceding whitespace (+4).
- `7de06b7` Reject directive without document body at EOS (+4).
- `60dceb4` Reject extra content after %YAML directive (+3).
- `bfc33b2` Implicit empty scalar for empty `---` document (+6).
- `5f0096d` Implicit empty scalar between back-to-back `---` (+2).
- `59fa0b0` Implicit empty scalar for `---\n...` empty document (+2).
- `0dbee5e` Relax over-strict double-tag check (net 0; wrong-reject -4).

Session 4 commits (341 → 373 = +32):

- `0bff770` Reject multi-line quoted scalars used as implicit keys (+6).
- `382a9a3` Reject multi-line plain scalars used as implicit keys (+6).
- `3df22e4` Close open collections before `---` starts new document (+12).
- `66597f2` Close open collections before final DocumentEnd at EOS (+6).
- `2f3830f` Close open collections before explicit `...` DocumentEnd (+2).

Session 5 commits (377 → 534 = +157):

- `265ea5a` Implement §8.1.1.2 block-scalar chomping (clip/strip/keep)
  and fix `find_block_scalar_indent` single-line bug (+47, biggest
  single-commit win since session 2).
- `74decb8` Preserve folded line breaks adjacent to more-indented
  content per §8.1.3.2 (+4).
- `71451c9` `check_for_mapping_ahead` scans past inner `:` so plain
  scalars containing colons are still recognised as mapping keys (+4).
- `c3d29e9` Synthesise implicit empty value when a key is followed by
  another key with no intervening `:` (yaml-test-suite 7W2P), via the
  new `innermost_mapping_has_odd_children` helper (+4).
- `b338eaa` Synthesise implicit empty key when a line starts with `:`
  (2JQS) — handle ImplicitDocumentStart / DocumentContent /
  BlockMappingKey-even states; skip the "this scalar is a new key"
  heuristic immediately after a just-synthesised empty key (+5).
  60.0% milestone reached.
- `efcc96c` Keep anchor on key when `BlockMappingStart` wraps an
  implicit key — distinguish root-position implicit-key (anchor
  goes to key) from value-position (anchor goes to mapping) (+10).
- `955dedf` Preserve literal whitespace beyond `content_indent` on
  blank lines in block scalars per §6.5 (+9).
- `60c1072` Line-aware "scalar is value vs new key" heuristic: skip
  the missing-value synthesis when the scalar shares a line with the
  most recent `:` (yaml-test-suite 6M2F) (+2).
- `21b947d` Reject unclosed flow collections at end-of-stream per
  §7.4 — `[ [ a, b, c ]` and friends (+2).
- `ddb83d2` Reject stray `]` / `}` outside flow context (+2).
- `6d505f8` Reject block-scalar indent indicator `|0` per
  §8.1.1.1 (+1).
- `22fd82a` Reject leading or double commas in flow collections (+2).
- `a0a8229` Block-scalar `content_indent` is leading-space count
  (no `base_indent + 1` floor) per §8.1.1.1; unblocks block-scalar
  content collection inside sequence items (net 0 passes, but
  failure mode for 4QFQ/M6YH/P2AD now advances one event).
- `d46528e` Drop the strict multiple-of-N indentation rule
  (§6.1 has no such requirement) (+2). 64.1%.
- `e3704b1` Pass `pending_anchor` / `pending_tag` to synthesised
  empty mapping keys so anchored empty keys (\`&a : a\`) don't
  leak into the next anchor check (net 0).
- `da173eb` Detect leading quoted scalars as implicit mapping
  keys — `check_for_mapping_ahead` scans past the leading quote
  (with `''` and `\"` escape handling) before searching for `:`;
  shared the BMS-opening logic into `maybe_open_block_mapping_for_key`.
  Fixes 6H3V, 6SLA, 87E4-like cases (+6).
- `262c98f` Trim trailing whitespace before plain-scalar fold so
  `a\nb  \n  c\nd\n\ne` folds to `a b c d\ne` instead of leaking
  the trailing spaces (+6). 65.7%.
- `ae48850` Reject implicit mapping key without `:` at end of
  stream — tracks `explicit_key_pending` to distinguish from
  spec-legal `? key` (+2).
- `3983360` Re-run `handle_indentation` after a block scalar so
  the next sibling `-` / `key:` dispatches against fresh
  `current_indent` (+6).
- `f942073` Synth empty value at BlockEnd when innermost map has
  odd children (yaml-test-suite 7W2P) (+3).
- `9039477` Synth empty value for key-only flow mapping entries
  at FlowEntry / FlowMappingEnd (+6). 68.0%.
- `9715e31` Fold multi-line plain scalars in flow context — break
  at `\n`/`\r` in the flow-content match arm + relaxed column rule
  for continuation in flow (+6).
- `394e21c` Block-scalar `base_indent` reads from `indent_stack`
  (not stale `current_indent`) so `|N` inside a sequence item
  resolves correctly (+4).
- `83795c9` Parent-aware multi-line plain scalar continuation per
  §6.5 / §7.3.3 — continuation must be at column N+2 or deeper
  (parent block at indent N has keys at N+1) (+14, single biggest
  win in this stretch).
- `0e94cc8` Implicit single-pair flow mapping inside flow sequence
  per §7.5 — track open implicit pairs in
  `implicit_flow_pair_depth` so `,` / `]` close them before
  continuing the outer sequence (+10). **72.7%** milestone reached.

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

**Path forward:** Surface errors, _then_ TDD each newly-exposed partial-parse
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

- **Quoted scalars at line head not detected as mapping keys** (26DV, 2EBW
  if quoted). Attempted the same `check_for_mapping_ahead` dispatch but
  regressed -2; needs scoping investigation.
- **Anchor-name `:` boundary** (2SXE). Currently includes `:` in anchor;
  spec is ambiguous here.
- **`---` followed by mapping on the same line** (9KBC, CXX2). Per the
  test suite, invalid; per some interpretations of the spec, valid.
- **Directives mid-stream without `...`** (9HCY). Validation written but
  blocked by D above.
- **Block-scalar header validation** (5LLU and others). Header is parsed but
  not validated against malformed indent indicators.
- **Tabs in indentation** (DK95/00, HS5T, 4EJS). Currently sometimes
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

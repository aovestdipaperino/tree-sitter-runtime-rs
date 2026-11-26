# Handoff: runtime port

State at checkpoint 2 (waves 0 to 3, `lexer.c` and `language.c` done). Everything below is committed in this repo.

## Done
Ported to Rust, each with differential tests against the C original (`rust/runtime/tests/`):
`point.c` + `point.h`, `length.h`, `alloc.c`, `array.h`, `reduce_action.h`, the `unicode.h`
decoders, `error_costs.h`/`host.h`/`ts_assert.h`/`endian.h` (constants and `cfg`). Struct layouts
(`api.rs`, `Length`, `ReduceAction`, `Array`) are checked field by field against C `sizeof` and
`offsetof` (`tests/layout.rs`). The mixed build (`ported.txt` lists the C files dropped) passes the
whole-library harness: 7,039 comparisons against the renamed C reference, 0 mismatches.

## How to verify
    cd rust
    cargo test -p tree-sitter-runtime-rs
    cargo clippy --all-targets -- -D warnings
    cargo build --release -p runtime-diff-harness && ./target/release/runtime-diff-harness

Note the harness is weak on `ts_point_edit` boundary cases (a seeded off-by-one there did not change
any corpus result); the unit test in `tests/helpers.rs` is what pins it.

## Checkpoint 2 additions
`lexer.c` (Rust; `Lexer` keeps the C layout, shared with the C parser) and `language.c` plus all the
inline helpers of `language.h` (including the lookahead iterator). `TSLanguage` and the `parser.h`
types are in `parser_abi.rs`. Differential tests: `tests/lexer.rs` runs 3,000 random operation
scripts (all four encodings, chunking, included ranges, logging) against the renamed C lexer and
compares full state after every step; `tests/language.rs` compares every function on every state and
symbol of the 8 real grammars. Seeded bugs in both are caught.
The variadic `TSLexer.log` cannot be written in stable Rust, so `runtime/c/lexer_log.c` (15 lines)
stays C; remove it when `c_variadic` stabilises. Latent C hazard reproduced as is: advancing a lexer
left at EOF with a non-null chunk (possible after an empty included range) reads one range past
the array; the test avoids that state.

## Subtree (checkpoint 3, first unit)
`subtree.rs`: `Subtree`/`MutableSubtree` are unions of an inline `u64` (bitfield accessors, little-endian
only) and a heap pointer; `SubtreeHeapData` keeps the C layout with its 11 flags as a `u16`. Tests:
bit layout against the C compiler for 200k random inline values and every heap flag; setters vs C bitfield
assignment; 3,000 lockstep random operation scripts (leaves, nodes, error nodes, edits, make_mut,
compare, strings, array ops, external scanner state) with the full state of every live tree compared
after each step through the C accessors; compress on left-leaning chains. Eight seeded mutations,
including repeat-depth and edit-boundary ones, are caught (one equivalent mutant, `>` vs `>=` on
repeat depth, cannot be). `subtree.c` is dropped from the C build, so the C stack/parser/tree code now
runs on the Rust subtree: the whole-library harness still shows 0 mismatches.
Known layout difference (harmless): C `SubtreeInlineData` has alignment 2, the port 8; it only occurs
inside the 8-aligned unions.

## Stack (checkpoint 3, second unit)
`stack.rs` mirrors `stack.c` structure for structure (`Stack` is opaque to C, so its layout is free;
slices, summaries and subtree arrays keep the C layout). `tests/stack.rs` drives the Rust and the C
stack with 1,500 lockstep random scripts (push/pop_count/pop_pending/pop_all/pop_error, copy,
remove, renumber, swap, pause/resume/halt, last external token, summaries, fork-and-merge, many-branch
merges up to the link limit, two-level merges, clear) and compares every observable of every version
after each step, plus normalized dot-graph output. Mutations in link limit, iterator limit, merge
state check, summary depth, error recovery cost and has-advanced are caught. Not catchable by
observation: node-pool size (memory only) and `==` vs `>=` in the pop-count callback (equivalent);
the `&&` in the both-have-errors merge shortcut is not reached because heads with differing error
costs cannot merge.
Latent C hazard reproduced: merging two versions whose paths contain null-subtree links to the same
node dereferences the null subtree (`ts_subtree_dynamic_precedence`); the test never merges in
scripts that push null subtrees. `stack.c` is now dropped from the C build.

## Tree (checkpoint 3, third unit)
`tree.rs` (`TSTree`, copy/delete/edit/included ranges/root nodes/changed ranges/dot graph) and the
`TreeCursor` types in `tree_cursor.rs`. `tests/tree.rs` compares it with the renamed C `tree.c` on
trees from the real parser: copy and ref counts, edits (also with included ranges), reparse and
changed ranges for 100 random edits, root node offsets, dot-graph output. Three seeded mutations caught.

## Tree cursor (checkpoint 3, fourth unit)
`tree_cursor.rs`: the whole cursor API plus the internal step functions and `current_status` (used by
queries). `tests/tree_cursor.rs`: 300 random walks per grammar over real parse trees of 8 grammars
(first/last child, next/previous sibling, parent, goto_descendant, first_child_for_byte/point, copy,
reset_to, reset) comparing the full cursor stack, current node, parent node, depth, descendant index,
field id/name and status after every step. Seeded mutations in sibling stop condition, alias
visibility, status walk, byte/point goal and previous-sibling position restore are caught.
Upstream bug reproduced on purpose: `child_iterator_previous` compares `(int8_t)child_index` with -1,
so a backward walk over a node with more than 255 children stops at child 511/255 (a 600-child node
built by hand stops after 87 steps; grammars never produce nodes that flat, since repeats are
balanced, which is why it is unseen in practice). Pinned by `previous_sibling_walk_over_wide_node_matches_c`.

## get_changed_ranges + reusable_node (checkpoint 3, fifth unit)
`get_changed_ranges.rs` (range edit, range-array diff, the tree iterator comparison) is compared with the
renamed C in `tests/ranges.rs` (200k random range edits, 50k range-array diffs) and, at tree level, in
`tests/tree.rs` (600 edit-reparse-changed-ranges rounds). Mutations in the has-changes check are caught;
three mutations (error-cost equality, external-token flag, final size comparison) are NOT: the generated
sources have no external-scanner tokens and few error nodes. `reusable_node.rs` is a header-only port with
no separate test yet; the parser port exercises it. Note: a mutation making `iterator_descend` use `>=` hangs.

## parser.c (ported, 2026-11-24)
`runtime/src/parser.rs`, one Rust function per C function, same order, raw pointers, `log!` macro
over libc `snprintf` so parse logs are byte-identical. Wasm branches still call the C stubs in
`wasm_store.c`. Verified only by the whole-library harness (`./target/release/runtime-diff-harness`,
9599 comparisons, 0 mismatches), which now also compares the parse and lex log text and a
progress-callback cancel-then-resume scenario. Seeded mutations caught: version limits, cost
difference, tree selection, missing-token insertion, recovery depth, callback op count. Not caught:
the operation-count scaling in `parser_balance_subtree` (needs a very long repetition cancelled
during balancing).

## node.c (ported, 2026-11-24)
`runtime/src/node.rs`, same structure as the C including the `goto` loop in `first_child_for_byte`.
Verified by the harness, whose tree dump now also records, for the first 150 nodes of every parse,
parent, siblings, children, first-child-for-byte, descendant-for-range (byte and point), every
field id, field names, `child_with_descendant` and `ts_node_edit`. 10 of 10 seeded mutations caught.

## query.c (ported, 2026-11-25)
Split over `query.rs` (types and helpers, written first), `query_analysis.rs`, `query_parse.rs` and
`query_cursor.rs` (the last three written in parallel by forked agents in git worktrees, then merged).
Query structs are Rust-layout (no C unit sees them); C bitfields are plain fields with explicit masks
(`CONSUMED_CAPTURE_COUNT_MASK` etc.). Verified only by the harness: 2468 queries compiled, 1076
rejected (error type and offset compared), 449k match/capture records, 17 cursor modes (match limit,
byte/point ranges, containing ranges, max start depth, remove_match, next_capture order, progress
callback, disabled capture/pattern). Harness queries: fixed list, generated from the grammar's node
and field names, and upstream highlights/tags/locals files in `harness/queries/`.
Mutations not caught, judged equivalent or out of reach: `should_descend` pruning comparisons (perf
only, the real start check is stricter), capture-order tie comparisons, `capture_list_pool_is_empty`
`>=`/`>` and `MAX_ANALYSIS_ITERATION_COUNT` (needs a pathological query).
Not covered: `ts_query_copy` (harness bindings are still 0.26; comes back with the binding fork).

## Next
the binding fork + tokensave test, then rust-anything
front-door/wire/lift as feasible. Medium/large grammar tiers after the runtime.

## Open questions for the user
- None open. ADR-011 and the v0.27.2 baseline were confirmed; the repo is public.

## Patched reference (ADR-013)
Reference C = pristine v0.27.2 + `rust/patches/*.patch`. Add a fix by adding a patch (generated
against `src/`, applied with `-p2`) and the matching Rust change; tests then compare to the patched
oracle. Rebased to v0.27.2 (ADR-013). TODO: Rust `write_to_string` is recursive while upstream is now iterative (deep trees overflow the stack).

## lib.c, atomic.h and the wasm stubs

`lib.c` is an amalgamation file the build never compiled, and no remaining C includes `atomic.h` (subtree.rs uses `core::sync::atomic`), so neither needed code. The 14 no-wasm dummies of `wasm_store.c` are now `rust/runtime/src/wasm_store.rs`; `wasm_store.c` is in `ported.txt`. The only C left in the build is `runtime/c/lexer_log.c` (ADR-012). Harness: 13143 compared, 0 mismatches.

## Binding fork and the tokensave test

`rust/binding` is the published `tree-sitter` 0.27.2 `binding_rust` (lib.rs, ffi.rs, bindings.rs, util.rs, wasm_*.rs) as a crate named `tree-sitter`, depending on `tree-sitter-runtime-rs` instead of building the C library. Differences from upstream: no `cc` build (`build.rs` only copies `stdlib-symbols.txt`), `extern crate tree_sitter_runtime_rs;` in lib.rs, `PARSER_HEADER` includes `../../../src/parser.h`, the `wasm` feature is an empty placeholder (ADR-003), no `links`. Smoke tests in `binding/tests/smoke.rs` (parse, edit, changed ranges, query).

Use it from another project with `[patch.crates-io] tree-sitter = { path = ".../rust/binding" }`; the semver must match, so tokensave (on `tree-sitter = "0.26"`) needs `"0.27"` and two `child_count()` call sites cast to `usize` (batch_extractor.rs:23, cobol_extractor.rs:413). Tested on a `git archive` copy of tokensave HEAD in the scratchpad (the working tree was untouched): release build succeeds, `cargo test --release --tests` all pass (lib 1077, a 3088-test suite, the rest green), zero failures.

Not done: the harness's own Rust-side bindings are still the 0.26 generation, so `ts_query_copy` stays uncovered; switch the harness to the `tree-sitter` fork to close it.

# Handoff: runtime port

State at checkpoint 1 (waves 0 to 3 done). Everything below is committed in this repo.

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

## Next (wave 4, checkpoints 2 and 3)
`lexer.c` + `language.c`, then the tangled core as one piece: `subtree`, `stack`, `tree`,
`tree_cursor`, `get_changed_ranges`, `parser`, then `node`. Risks to read first in `MAP.md`:
`Subtree` bitfields and low-bit pointer tagging need hand-written accessors with layout tests;
C and Rust units must share those structs while mixed. `query.c` stays C until the last wave.

## Open questions for the user
- ADR-011 (mixed build, one unit at a time) is proposed, not confirmed.
- ADR-001 ports tree-sitter 0.26.13, not tokensave's pinned 0.26.9: flagged for veto.
- No GitHub remote exists for this repo yet; creating a public repo needs a go-ahead.

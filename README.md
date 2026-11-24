# tree-sitter-runtime-rs

A port of the [tree-sitter](https://github.com/tree-sitter/tree-sitter) C runtime (0.26.13) to Rust,
exporting the same C ABI so the `tree-sitter` bindings and every grammar link unchanged.

Status: in progress. See [rust/MAP.md](rust/MAP.md) for the plan and state,
[rust/DECISIONS.md](rust/DECISIONS.md) for the decisions, and
[rust/GLOSSARY.md](rust/GLOSSARY.md) for the vocabulary.

`src/`, `include/` and `binding_rust/` are the unmodified upstream 0.26.13 sources, kept as the
reference the Rust port is verified against (MIT, see `LICENSE.tree-sitter`).

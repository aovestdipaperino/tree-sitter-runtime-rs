# Glossary

The runtime's domain terms, what they mean, and their name in Rust. Add to it as units bring new terms.

| C | Meaning | Rust |
|---|---|---|
| `TSPoint` | public row/column pair (`u32` each) | `TSPoint`, `#[repr(C)]` |
| `TSRange`, `TSInput`, `TSLogger`, `TSInputEdit` | public structs of `api.h` | same names, `#[repr(C)]`, layout frozen (ADR-002) |
| `TSNode` | public node handle: `context[4]`, `id`, `tree` | `TSNode`, `#[repr(C)]` |
| `TSLanguage` | pure data a grammar exports (tables, lex fns, scanner) | `TSLanguage`, mirrors `parser.h` ABI 15 (same type as in tree-sitter-lite-rs) |
| `TSParser` | the parser; defined inside `parser.c` | `TSParser` (internal until `parser.c` is ported) |
| `TSTree` | a parse tree: root `Subtree`, language, included ranges | `TSTree` |
| `Length` | `{bytes: u32, extent: TSPoint}`, a distance in both measures | `Length` |
| `Subtree` | tagged union: a pointer to `SubtreeHeapData`, or inline data when the pointer's low bit is 1 | `Subtree` (union), low-bit tag helpers |
| `SubtreeInlineData` | compact form for small non-error leaves; **C bitfields** | `SubtreeInline` with explicit bit accessors, checked against C |
| `SubtreeHeapData` | heap form: ref count, padding, size, flags (**bitfields**), children or external state | `SubtreeHeap` |
| `MutableSubtree` | like `Subtree` but known uniquely owned | `MutableSubtree` |
| `SubtreePool` | free lists of subtree allocations | `SubtreePool` |
| `ExternalScannerState` | serialized scanner state: inline up to 24 bytes, else heap | `ExternalScannerState` |
| `Array(T)` | the runtime's generic dynamic array macro: `{contents, size, capacity}` | `Array<T>`, `#[repr(C)]`, same layout while C still shares it |
| `Lexer` | `TSLexer` plus positions, current chunk, included ranges | `Lexer` |
| `Stack`, `StackNode`, `StackLink`, `StackHead`, `StackSlice`, `StackSummary` | the GLR parse stack: a graph of versions | same names in `stack` |
| `ReusableNode` | cursor over the previous tree while reusing unchanged subtrees | `ReusableNode` |
| `TSTreeCursor`, `TreeCursor` | public (opaque) and internal tree cursor | same |
| `ts_malloc` family | allocation hooks, replaceable with `ts_set_allocator` | `alloc` module, same hook semantics |
| error cost | numbers (`error_costs.h`) deciding which version of the stack survives | `error_costs`, constants identical |

## Traps (from the C)

- `Subtree` pointer tagging and bitfield layout depend on endianness and pointer size
  (`TS_BIG_ENDIAN`, `TS_PTR_SIZE`); the Rust mirror targets little-endian 64-bit first.
- `SubtreeHeapData.ref_count` is `volatile` and updated with the runtime's own atomics (`atomic.h`).
- `LENGTH_UNDEFINED` is `{0, {0, 1}}`: a zero byte count with a non-zero column.
- Arithmetic on `Length`/`TSPoint` is unsigned and relies on wrapping or saturating exactly as in
  `length.h` and `point.h`; reproduce, do not "fix".

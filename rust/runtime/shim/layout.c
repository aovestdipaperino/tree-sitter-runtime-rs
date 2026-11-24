// Test shim: the C `sizeof`, `_Alignof` and `offsetof` of every struct whose layout the Rust
// port mirrors (tests/layout.rs compares them with the Rust values).
#include <stddef.h>
#include <string.h>
#include "tree_sitter/api.h"
#include "length.h"
#include "reduce_action.h"

typedef struct {
  const char *key;
  size_t value;
} Entry;

#define SZ(T) {#T ".sizeof", sizeof(T)}, {#T ".alignof", _Alignof(T)}
#define OFF(T, F) {#T "." #F, offsetof(T, F)}

static const Entry TABLE[] = {
  SZ(TSPoint), OFF(TSPoint, row), OFF(TSPoint, column),
  SZ(TSRange), OFF(TSRange, start_point), OFF(TSRange, end_point), OFF(TSRange, start_byte), OFF(TSRange, end_byte),
  SZ(TSInput), OFF(TSInput, payload), OFF(TSInput, read), OFF(TSInput, encoding), OFF(TSInput, decode),
  SZ(TSParseState), OFF(TSParseState, payload), OFF(TSParseState, current_byte_offset), OFF(TSParseState, has_error),
  SZ(TSParseOptions), OFF(TSParseOptions, payload), OFF(TSParseOptions, progress_callback),
  SZ(TSLogger), OFF(TSLogger, payload), OFF(TSLogger, log),
  SZ(TSInputEdit), OFF(TSInputEdit, start_byte), OFF(TSInputEdit, old_end_byte), OFF(TSInputEdit, new_end_byte),
    OFF(TSInputEdit, start_point), OFF(TSInputEdit, old_end_point), OFF(TSInputEdit, new_end_point),
  SZ(TSNode), OFF(TSNode, context), OFF(TSNode, id), OFF(TSNode, tree),
  SZ(TSTreeCursor), OFF(TSTreeCursor, tree), OFF(TSTreeCursor, id), OFF(TSTreeCursor, context),
  SZ(TSQueryCapture), OFF(TSQueryCapture, node), OFF(TSQueryCapture, index),
  SZ(TSQueryMatch), OFF(TSQueryMatch, id), OFF(TSQueryMatch, pattern_index), OFF(TSQueryMatch, capture_count), OFF(TSQueryMatch, captures),
  SZ(TSQueryPredicateStep), OFF(TSQueryPredicateStep, type), OFF(TSQueryPredicateStep, value_id),
  SZ(TSQueryCursorState), OFF(TSQueryCursorState, payload), OFF(TSQueryCursorState, current_byte_offset),
  SZ(TSQueryCursorOptions), OFF(TSQueryCursorOptions, payload), OFF(TSQueryCursorOptions, progress_callback),
  SZ(ReduceAction), OFF(ReduceAction, count), OFF(ReduceAction, symbol), OFF(ReduceAction, dynamic_precedence), OFF(ReduceAction, production_id),
  SZ(ReduceActionSet), OFF(ReduceActionSet, contents), OFF(ReduceActionSet, size), OFF(ReduceActionSet, capacity),
  SZ(Length), OFF(Length, bytes), OFF(Length, extent),
  SZ(TSLanguageMetadata), OFF(TSLanguageMetadata, major_version), OFF(TSLanguageMetadata, minor_version), OFF(TSLanguageMetadata, patch_version),
};

size_t shim_layout_len(void) { return sizeof(TABLE) / sizeof(TABLE[0]); }
const char *shim_layout_key(size_t i) { return TABLE[i].key; }
size_t shim_layout_value(size_t i) { return TABLE[i].value; }

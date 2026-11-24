// Test shim: the C `sizeof`, `_Alignof` and `offsetof` of every struct whose layout the Rust
// port mirrors (tests/layout.rs compares them with the Rust values).
#include <stddef.h>
#include <string.h>
#include "tree_sitter/api.h"
#include "length.h"
#include "reduce_action.h"
#include "lexer.h"
#include "parser.h"

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
  SZ(TSLexer), OFF(TSLexer, lookahead), OFF(TSLexer, result_symbol), OFF(TSLexer, advance), OFF(TSLexer, mark_end),
    OFF(TSLexer, get_column), OFF(TSLexer, is_at_included_range_start), OFF(TSLexer, eof), OFF(TSLexer, log),
  SZ(Lexer), OFF(Lexer, data), OFF(Lexer, current_position), OFF(Lexer, token_start_position), OFF(Lexer, token_end_position),
    OFF(Lexer, included_ranges), OFF(Lexer, chunk), OFF(Lexer, input), OFF(Lexer, logger), OFF(Lexer, included_range_count),
    OFF(Lexer, current_included_range_index), OFF(Lexer, chunk_start), OFF(Lexer, chunk_size), OFF(Lexer, lookahead_size),
    OFF(Lexer, did_get_column), OFF(Lexer, column_data), OFF(Lexer, debug_buffer),
  SZ(ColumnData), SZ(TSFieldMapEntry), SZ(TSMapSlice), SZ(TSSymbolMetadata), SZ(TSLexerMode), SZ(TSCharacterRange), SZ(TSParseAction), SZ(TSParseActionEntry),
  SZ(TSLanguage), OFF(TSLanguage, abi_version), OFF(TSLanguage, symbol_count), OFF(TSLanguage, alias_count), OFF(TSLanguage, token_count),
    OFF(TSLanguage, external_token_count), OFF(TSLanguage, state_count), OFF(TSLanguage, large_state_count), OFF(TSLanguage, production_id_count),
    OFF(TSLanguage, field_count), OFF(TSLanguage, max_alias_sequence_length), OFF(TSLanguage, parse_table), OFF(TSLanguage, small_parse_table),
    OFF(TSLanguage, small_parse_table_map), OFF(TSLanguage, parse_actions), OFF(TSLanguage, symbol_names), OFF(TSLanguage, field_names),
    OFF(TSLanguage, field_map_slices), OFF(TSLanguage, field_map_entries), OFF(TSLanguage, symbol_metadata), OFF(TSLanguage, public_symbol_map),
    OFF(TSLanguage, alias_map), OFF(TSLanguage, alias_sequences), OFF(TSLanguage, lex_modes), OFF(TSLanguage, lex_fn), OFF(TSLanguage, keyword_lex_fn),
    OFF(TSLanguage, keyword_capture_token), OFF(TSLanguage, external_scanner), OFF(TSLanguage, primary_state_ids), OFF(TSLanguage, name),
    OFF(TSLanguage, reserved_words), OFF(TSLanguage, max_reserved_word_set_size), OFF(TSLanguage, supertype_count), OFF(TSLanguage, supertype_symbols),
    OFF(TSLanguage, supertype_map_slices), OFF(TSLanguage, supertype_map_entries), OFF(TSLanguage, metadata),
  SZ(Length), OFF(Length, bytes), OFF(Length, extent),
  SZ(TSLanguageMetadata), OFF(TSLanguageMetadata, major_version), OFF(TSLanguageMetadata, minor_version), OFF(TSLanguageMetadata, patch_version),
};

size_t shim_layout_len(void) { return sizeof(TABLE) / sizeof(TABLE[0]); }
const char *shim_layout_key(size_t i) { return TABLE[i].key; }
size_t shim_layout_value(size_t i) { return TABLE[i].value; }

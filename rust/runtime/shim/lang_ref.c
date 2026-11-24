// Test shim: wrappers over the inline helpers of language.h (the exported functions of language.c
// are compiled in this archive with renamed symbols, see build.rs).
#include "language.h"

uint16_t c_lookup(const TSLanguage *l, TSStateId s, TSSymbol y) { return ts_language_lookup(l, s, y); }
bool c_has_actions(const TSLanguage *l, TSStateId s, TSSymbol y) { return ts_language_has_actions(l, s, y); }
const TSParseAction *c_actions(const TSLanguage *l, TSStateId s, TSSymbol y, uint32_t *n) { return ts_language_actions(l, s, y, n); }
bool c_has_reduce(const TSLanguage *l, TSStateId s, TSSymbol y) { return ts_language_has_reduce_action(l, s, y); }
bool c_state_is_primary(const TSLanguage *l, TSStateId s) { return ts_language_state_is_primary(l, s); }
const bool *c_enabled_external_tokens(const TSLanguage *l, unsigned s) { return ts_language_enabled_external_tokens(l, s); }
const TSSymbol *c_alias_sequence(const TSLanguage *l, uint32_t p) { return ts_language_alias_sequence(l, p); }
TSSymbol c_alias_at(const TSLanguage *l, uint32_t p, uint32_t c) { return ts_language_alias_at(l, p, c); }
void c_field_map(const TSLanguage *l, uint32_t p, const TSFieldMapEntry **a, const TSFieldMapEntry **b) { ts_language_field_map(l, p, a, b); }
void c_aliases_for_symbol(const TSLanguage *l, TSSymbol o, const TSSymbol **a, const TSSymbol **b) { ts_language_aliases_for_symbol(l, o, a, b); }
void c_lookaheads(const TSLanguage *l, TSStateId s, LookaheadIterator *out) { *out = ts_language_lookaheads(l, s); }
bool c_lookahead_next(LookaheadIterator *it) { return ts_lookahead_iterator__next(it); }

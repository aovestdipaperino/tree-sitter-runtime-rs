// Test shim for the subtree port: the C accessors (header inline functions, which read the bitfields
// with the C compiler's layout) exposed as a flat record, so the Rust accessors can be compared.
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include "subtree.h"

#define N_SLOTS 40

// One node's observable properties, through the C header's accessors.
void shim_dump_node(Subtree s, uint64_t *o) {
  memset(o, 0, N_SLOTS * sizeof(uint64_t));
  bool heap = !s.data.is_inline;
  o[0] = s.data.is_inline;
  o[1] = ts_subtree_symbol(s);
  o[2] = ts_subtree_parse_state(s);
  o[3] = ts_subtree_visible(s);
  o[4] = ts_subtree_named(s);
  o[5] = ts_subtree_extra(s);
  o[6] = ts_subtree_has_changes(s);
  o[7] = ts_subtree_missing(s);
  o[8] = ts_subtree_is_keyword(s);
  o[9] = ts_subtree_lookahead_bytes(s);
  Length p = ts_subtree_padding(s), z = ts_subtree_size(s);
  o[10] = p.bytes; o[11] = p.extent.row; o[12] = p.extent.column;
  o[13] = z.bytes; o[14] = z.extent.row; o[15] = z.extent.column;
  o[16] = ts_subtree_child_count(s);
  o[17] = ts_subtree_error_cost(s);
  o[18] = ts_subtree_visible_child_count(s);
  o[19] = (heap && s.ptr->child_count > 0) ? s.ptr->named_child_count : 0;
  o[20] = ts_subtree_visible_descendant_count(s);
  o[21] = (uint64_t)(int64_t)ts_subtree_dynamic_precedence(s);
  o[22] = ts_subtree_repeat_depth(s);
  o[23] = ts_subtree_production_id(s);
  o[24] = ts_subtree_fragile_left(s);
  o[25] = ts_subtree_fragile_right(s);
  o[26] = ts_subtree_has_external_tokens(s);
  o[27] = ts_subtree_has_external_scanner_state_change(s);
  o[28] = ts_subtree_depends_on_column(s);
  o[29] = ts_subtree_leaf_symbol(s);
  o[30] = ts_subtree_leaf_parse_state(s);
  o[31] = heap ? s.ptr->ref_count : 0;
  o[32] = ts_subtree_total_bytes(s);
  o[33] = ts_subtree_is_repetition(s);
  o[34] = ts_subtree_is_fragile(s);
  o[35] = ts_subtree_is_error(s);
  o[36] = ts_subtree_is_eof(s);
  const ExternalScannerState *e = ts_subtree_external_scanner_state(s);
  o[37] = e->length;
  uint64_t h = 1469598103934665603ULL;
  const char *d = ts_external_scanner_state_data(e);
  for (uint32_t i = 0; i < e->length; i++) h = (h ^ (uint8_t)d[i]) * 1099511628211ULL;
  o[38] = h;
  o[39] = (heap && s.ptr->child_count == 0 && ts_subtree_is_error(s)) ? (uint64_t)(uint32_t)s.ptr->lookahead_char : 0;
}

Subtree shim_child(Subtree s, uint32_t i) { return ts_subtree_children(s)[i]; }

// Bit position of each heap flag: sets one bitfield in a zeroed struct and returns the raw 16 bits
// stored at the flags' byte offset (bytes 44 and 45).
uint16_t shim_heap_flag_bits(int which) {
  SubtreeHeapData d;
  memset(&d, 0, sizeof d);
  switch (which) {
    case 0: d.visible = 1; break;
    case 1: d.named = 1; break;
    case 2: d.extra = 1; break;
    case 3: d.fragile_left = 1; break;
    case 4: d.fragile_right = 1; break;
    case 5: d.has_changes = 1; break;
    case 6: d.has_external_tokens = 1; break;
    case 7: d.has_external_scanner_state_change = 1; break;
    case 8: d.depends_on_column = 1; break;
    case 9: d.is_missing = 1; break;
    case 10: d.is_keyword = 1; break;
  }
  uint16_t out;
  memcpy(&out, (char *)&d + offsetof(SubtreeHeapData, parse_state) + 2, 2);
  return out;
}

Subtree shim_inline_from_bits(uint64_t bits) {
  Subtree s;
  memcpy(&s, &bits, 8);
  return s;
}

// Builds an inline subtree in C from field values (bitfield assignment, truncating like C does).
uint64_t shim_inline_build(uint16_t parse_state, uint8_t symbol, uint8_t pcols, uint8_t prows, uint8_t pbytes, uint8_t sbytes,
                           uint8_t lookahead, int visible, int named, int extra, int changes, int missing, int keyword) {
  Subtree s;
  memset(&s, 0, sizeof s);
  s.data.parse_state = parse_state; s.data.symbol = symbol; s.data.padding_columns = pcols; s.data.padding_rows = prows;
  s.data.padding_bytes = pbytes; s.data.size_bytes = sbytes; s.data.lookahead_bytes = lookahead;
  s.data.visible = visible; s.data.named = named; s.data.extra = extra; s.data.has_changes = changes;
  s.data.is_missing = missing; s.data.is_keyword = keyword; s.data.is_inline = 1;
  uint64_t bits;
  memcpy(&bits, &s, 8);
  return bits;
}

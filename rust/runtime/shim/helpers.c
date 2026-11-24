// Test shim: exposes the header-only `static inline` helpers of the C runtime as real functions,
// so the Rust ports can be compared with them directly (tests/helpers.rs).
#include <stdint.h>
#include "point.h"
#include "length.h"

TSPoint shim_point_add(TSPoint a, TSPoint b) { return point_add(a, b); }
TSPoint shim_point_sub(TSPoint a, TSPoint b) { return point_sub(a, b); }
bool shim_point_lte(TSPoint a, TSPoint b) { return point_lte(a, b); }
bool shim_point_lt(TSPoint a, TSPoint b) { return point_lt(a, b); }
bool shim_point_gt(TSPoint a, TSPoint b) { return point_gt(a, b); }
bool shim_point_gte(TSPoint a, TSPoint b) { return point_gte(a, b); }
bool shim_point_eq(TSPoint a, TSPoint b) { return point_eq(a, b); }

Length shim_length_add(Length a, Length b) { return length_add(a, b); }
Length shim_length_sub(Length a, Length b) { return length_sub(a, b); }
Length shim_length_min(Length a, Length b) { return length_min(a, b); }
Length shim_length_zero(void) { return length_zero(); }
Length shim_length_saturating_sub(Length a, Length b) { return length_saturating_sub(a, b); }
bool shim_length_is_undefined(Length a) { return length_is_undefined(a); }
Length shim_length_undefined(void) { return LENGTH_UNDEFINED; }
Length shim_length_max(void) { return LENGTH_MAX; }

#include "unicode.h"
uint32_t shim_decode_utf8(const uint8_t *s, uint32_t n, int32_t *cp) { return ts_decode_utf8(s, n, cp); }
uint32_t shim_decode_utf16_le(const uint8_t *s, uint32_t n, int32_t *cp) { return ts_decode_utf16_le(s, n, cp); }
uint32_t shim_decode_utf16_be(const uint8_t *s, uint32_t n, int32_t *cp) { return ts_decode_utf16_be(s, n, cp); }

// ---- array.h: the private _array__* helpers, on raw parts ----
#include "array.h"
void shim_array_erase(void *c, uint32_t *size, size_t es, uint32_t i) { _array__erase(c, size, es, i); }
void *shim_array_reserve(void *c, uint32_t *cap, size_t es, uint32_t n) { return _array__reserve(c, cap, es, n); }
void *shim_array_assign(void *c, uint32_t *sz, uint32_t *cap, const void *oc, uint32_t os, size_t es) {
  return _array__assign(c, sz, cap, oc, os, es);
}
void *shim_array_grow(void *c, uint32_t sz, uint32_t *cap, uint32_t n, size_t es) { return _array__grow(c, sz, cap, n, es); }
void *shim_array_splice(void *c, uint32_t *sz, uint32_t *cap, size_t es, uint32_t i, uint32_t oc, uint32_t nc, const void *el) {
  return _array__splice(c, sz, cap, es, i, oc, nc, el);
}

typedef struct { uint32_t key; uint16_t tag; } SortElem;
typedef Array(SortElem) SortArray;
static int cmp_key(const SortElem *a, const uint32_t *needle) { return (int)a->key - (int)*needle; }
// Returns index; writes `exists`.
uint32_t shim_array_search_sorted_by(const SortElem *contents, uint32_t size, uint32_t needle, bool *exists_out) {
  SortArray a = {(SortElem *)contents, size, size};
  unsigned index, exists;
  array_search_sorted_by(&a, .key, needle, &index, &exists);
  *exists_out = exists;
  return index;
}
uint32_t shim_array_search_sorted_with(const SortElem *contents, uint32_t size, uint32_t needle, bool *exists_out) {
  SortArray a = {(SortElem *)contents, size, size};
  unsigned index, exists;
  array_search_sorted_with(&a, cmp_key, &needle, &index, &exists);
  *exists_out = exists;
  return index;
}

#include "reduce_action.h"
// Adds to a set held as raw parts; returns the (possibly moved) contents.
ReduceAction *shim_reduce_action_set_add(ReduceAction *contents, uint32_t *size, uint32_t *cap, ReduceAction a) {
  ReduceActionSet s = {contents, *size, *cap};
  ts_reduce_action_set_add(&s, a);
  *size = s.size;
  *cap = s.capacity;
  return s.contents;
}

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

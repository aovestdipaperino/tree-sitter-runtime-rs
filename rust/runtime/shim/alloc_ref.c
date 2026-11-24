// Test shim: the C allocator unit (src/alloc.c, compiled with its symbols renamed by build.rs)
// plus accessors, so tests can drive it next to the Rust port.
#include <stddef.h>
#include "alloc.h"

// Declared in api.h upstream; the renamed reference symbol needs its own prototype here.
void ts_set_allocator(void *(*)(size_t), void *(*)(size_t, size_t), void *(*)(void *, size_t), void (*)(void *));

void c_set_allocator(void *(*m)(size_t), void *(*c)(size_t, size_t), void *(*r)(void *, size_t), void (*f)(void *)) {
  ts_set_allocator(m, c, r, f);
}
void *c_malloc(size_t n) { return ts_current_malloc(n); }
void *c_calloc(size_t n, size_t s) { return ts_current_calloc(n, s); }
void *c_realloc(void *p, size_t n) { return ts_current_realloc(p, n); }
void c_free(void *p) { ts_current_free(p); }

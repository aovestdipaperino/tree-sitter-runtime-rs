//! Port of `src/alloc.c`: the allocator hooks (`ts_set_allocator`).
//!
//! The four `ts_current_*` function pointers are exported data symbols: the C units that are not
//! ported yet call through them (via the `ts_malloc`/`ts_calloc`/... macros in `alloc.h`), and so
//! do the ported ones.
use core::ffi::c_void;

unsafe extern "C" {
    fn malloc(size: usize) -> *mut c_void;
    fn calloc(count: usize, size: usize) -> *mut c_void;
    fn realloc(buffer: *mut c_void, size: usize) -> *mut c_void;
    fn free(buffer: *mut c_void);
    fn abort() -> !;
}

type MallocFn = unsafe extern "C" fn(usize) -> *mut c_void;
type CallocFn = unsafe extern "C" fn(usize, usize) -> *mut c_void;
type ReallocFn = unsafe extern "C" fn(*mut c_void, usize) -> *mut c_void;
type FreeFn = unsafe extern "C" fn(*mut c_void);

/// Fails like the C does: a message without a newline on stderr, then `abort`.
fn out_of_memory(what: &str, bytes: usize) -> ! {
    eprint!("tree-sitter failed to {what} {bytes} bytes");
    // SAFETY: `abort` has no preconditions.
    unsafe { abort() }
}

unsafe extern "C" fn ts_malloc_default(size: usize) -> *mut c_void {
    // SAFETY: plain libc call; any size is allowed.
    let result = unsafe { malloc(size) };
    if size > 0 && result.is_null() {
        out_of_memory("allocate", size);
    }
    result
}

unsafe extern "C" fn ts_calloc_default(count: usize, size: usize) -> *mut c_void {
    // SAFETY: plain libc call.
    let result = unsafe { calloc(count, size) };
    if count > 0 && result.is_null() {
        // The C multiplies unsigned `size_t`s, wrapping.
        out_of_memory("allocate", count.wrapping_mul(size));
    }
    result
}

unsafe extern "C" fn ts_realloc_default(buffer: *mut c_void, size: usize) -> *mut c_void {
    // SAFETY: the caller passes a pointer from this allocator (or null), as in the C.
    let result = unsafe { realloc(buffer, size) };
    if size > 0 && result.is_null() {
        out_of_memory("reallocate", size);
    }
    result
}

// Allow clients to override allocation functions dynamically.
#[unsafe(no_mangle)]
pub static mut ts_current_malloc: Option<MallocFn> = Some(ts_malloc_default);
#[unsafe(no_mangle)]
pub static mut ts_current_calloc: Option<CallocFn> = Some(ts_calloc_default);
#[unsafe(no_mangle)]
pub static mut ts_current_realloc: Option<ReallocFn> = Some(ts_realloc_default);
#[unsafe(no_mangle)]
pub static mut ts_current_free: Option<FreeFn> = Some(free);

/// Replaces the allocator; a null function selects the default for that operation.
///
/// # Safety
/// Not thread-safe, like the C: call it before any other tree-sitter function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_set_allocator(
    new_malloc: Option<MallocFn>,
    new_calloc: Option<CallocFn>,
    new_realloc: Option<ReallocFn>,
    new_free: Option<FreeFn>,
) {
    // SAFETY: see the function's contract; these are plain writes of function pointers.
    unsafe {
        ts_current_malloc = Some(new_malloc.unwrap_or(ts_malloc_default));
        ts_current_calloc = Some(new_calloc.unwrap_or(ts_calloc_default));
        ts_current_realloc = Some(new_realloc.unwrap_or(ts_realloc_default));
        ts_current_free = Some(new_free.unwrap_or(free));
    }
}

/// `ts_malloc` from `alloc.h`: calls the current hook.
///
/// # Safety
/// Same contract as `malloc`.
#[inline]
pub unsafe fn ts_malloc(size: usize) -> *mut c_void {
    // SAFETY: the hooks are initialised statically and only replaced by `ts_set_allocator`.
    unsafe { (ts_current_malloc.unwrap_unchecked())(size) }
}

/// `ts_calloc` from `alloc.h`.
///
/// # Safety
/// Same contract as `calloc`.
#[inline]
pub unsafe fn ts_calloc(count: usize, size: usize) -> *mut c_void {
    // SAFETY: as for `ts_malloc`.
    unsafe { (ts_current_calloc.unwrap_unchecked())(count, size) }
}

/// `ts_realloc` from `alloc.h`.
///
/// # Safety
/// Same contract as `realloc`.
#[inline]
pub unsafe fn ts_realloc(buffer: *mut c_void, size: usize) -> *mut c_void {
    // SAFETY: as for `ts_malloc`.
    unsafe { (ts_current_realloc.unwrap_unchecked())(buffer, size) }
}

/// `ts_free` from `alloc.h`.
///
/// # Safety
/// Same contract as `free`.
#[inline]
pub unsafe fn ts_free(buffer: *mut c_void) {
    // SAFETY: as for `ts_malloc`.
    unsafe { (ts_current_free.unwrap_unchecked())(buffer) }
}

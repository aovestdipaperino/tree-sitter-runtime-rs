//! Port of `src/array.h`: the growable array shared by almost every unit.
//!
//! `Array<T>` has the C layout of `Array(T)` (`{ T *contents; uint32_t size; uint32_t capacity; }`)
//! so it can sit inside structs that C units still access. The operations follow the C
//! `_array__*` helpers line by line, including the growth policy (capacity doubles, minimum 8),
//! unsigned wraparound, zero-filled `grow_by`, and `memmove`/`memcpy` semantics: elements are
//! moved bitwise and are never dropped or cloned, which is what the C code relies on. For that
//! reason `T` must be a plain-data type (`Copy`).
use crate::alloc::{ts_free, ts_malloc, ts_realloc};
use core::ffi::c_void;
use core::ptr;

/// `ts_assert` from `ts_assert.h`: the C build keeps assertions on (no `NDEBUG`).
#[macro_export]
macro_rules! ts_assert {
    ($e:expr) => {
        assert!($e)
    };
}

#[repr(C)]
pub struct Array<T: Copy> {
    pub contents: *mut T,
    pub size: u32,
    pub capacity: u32,
}

impl<T: Copy> Clone for Array<T> {
    /// A shallow copy of the header, as C struct assignment is.
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: Copy> Copy for Array<T> {}

impl<T: Copy> Default for Array<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ---- The `_array__*` helpers, on raw parts ----

/// `_array__erase`.
///
/// # Safety
/// `contents` must point to `*size` valid elements of `element_size` bytes.
pub unsafe fn raw_erase(contents: *mut u8, size: &mut u32, element_size: usize, index: u32) {
    ts_assert!(index < *size);
    // SAFETY: index < size, so both ranges are inside the allocation.
    unsafe {
        ptr::copy(
            contents.add((index as usize + 1) * element_size),
            contents.add(index as usize * element_size),
            ((*size - index - 1) as usize) * element_size,
        );
    }
    *size -= 1;
}

/// `_array__reserve`.
///
/// # Safety
/// `contents` is null or a live allocation from the `ts_malloc` family.
pub unsafe fn raw_reserve(
    contents: *mut u8,
    capacity: &mut u32,
    element_size: usize,
    new_capacity: u32,
) -> *mut u8 {
    let mut new_contents = contents;
    if new_capacity > *capacity {
        let bytes = new_capacity as usize * element_size;
        // SAFETY: forwarded contract.
        new_contents = unsafe {
            if contents.is_null() {
                ts_malloc(bytes)
            } else {
                ts_realloc(contents as *mut c_void, bytes)
            }
        } as *mut u8;
        *capacity = new_capacity;
    }
    new_contents
}

/// `_array__assign`.
///
/// # Safety
/// `other_contents` points to `other_size` valid elements; the rest as for `raw_reserve`.
pub unsafe fn raw_assign(
    self_contents: *mut u8,
    self_size: &mut u32,
    self_capacity: &mut u32,
    other_contents: *const u8,
    other_size: u32,
    element_size: usize,
) -> *mut u8 {
    // SAFETY: forwarded contract.
    let new_contents =
        unsafe { raw_reserve(self_contents, self_capacity, element_size, other_size) };
    *self_size = other_size;
    // SAFETY: the destination has room for `other_size` elements; ranges may not overlap (distinct arrays).
    unsafe {
        ptr::copy_nonoverlapping(
            other_contents,
            new_contents,
            *self_size as usize * element_size,
        )
    };
    new_contents
}

/// `_array__grow`.
///
/// # Safety
/// As for `raw_reserve`.
pub unsafe fn raw_grow(
    contents: *mut u8,
    size: u32,
    capacity: &mut u32,
    count: u32,
    element_size: usize,
) -> *mut u8 {
    let mut new_contents = contents;
    let new_size = size.wrapping_add(count);
    if new_size > *capacity {
        let mut new_capacity = capacity.wrapping_mul(2);
        if new_capacity < 8 {
            new_capacity = 8;
        }
        if new_capacity < new_size {
            new_capacity = new_size;
        }
        // SAFETY: forwarded contract.
        new_contents = unsafe { raw_reserve(contents, capacity, element_size, new_capacity) };
    }
    new_contents
}

/// `_array__splice`.
///
/// # Safety
/// `elements` is null (zero-fill) or points to `new_count` valid elements that do not alias the
/// array; the rest as for `raw_reserve`.
#[allow(clippy::too_many_arguments)]
pub unsafe fn raw_splice(
    self_contents: *mut u8,
    size: &mut u32,
    capacity: &mut u32,
    element_size: usize,
    index: u32,
    old_count: u32,
    new_count: u32,
    elements: *const u8,
) -> *mut u8 {
    let new_size = size.wrapping_add(new_count).wrapping_sub(old_count);
    let old_end = index.wrapping_add(old_count);
    let new_end = index.wrapping_add(new_count);
    ts_assert!(old_end <= *size);

    // SAFETY: forwarded contract.
    let contents = unsafe { raw_reserve(self_contents, capacity, element_size, new_size) };
    // SAFETY: old_end <= size <= capacity after the reserve, and the tail moves within the buffer.
    unsafe {
        if *size > old_end {
            ptr::copy(
                contents.add(old_end as usize * element_size),
                contents.add(new_end as usize * element_size),
                (*size - old_end) as usize * element_size,
            );
        }
        if new_count > 0 {
            let dest = contents.add(index as usize * element_size);
            let bytes = new_count as usize * element_size;
            if elements.is_null() {
                ptr::write_bytes(dest, 0, bytes);
            } else {
                ptr::copy_nonoverlapping(elements, dest, bytes);
            }
        }
    }
    *size = size.wrapping_add(new_count).wrapping_sub(old_count);
    contents
}

impl<T: Copy> Array<T> {
    const ELEM: usize = size_of::<T>();

    /// `array_new` / `array_init`.
    pub const fn new() -> Self {
        Array {
            contents: ptr::null_mut(),
            size: 0,
            capacity: 0,
        }
    }

    /// `array_get`.
    ///
    /// # Safety
    /// The array must be consistent (`contents` valid for `size` elements).
    pub unsafe fn get(&self, index: u32) -> *mut T {
        ts_assert!(index < self.size);
        // SAFETY: bounds checked above.
        unsafe { self.contents.add(index as usize) }
    }

    /// `array_front`.
    ///
    /// # Safety
    /// As for `get`.
    pub unsafe fn front(&self) -> *mut T {
        unsafe { self.get(0) }
    }

    /// `array_back`.
    ///
    /// # Safety
    /// As for `get`.
    pub unsafe fn back(&self) -> *mut T {
        unsafe { self.get(self.size.wrapping_sub(1)) }
    }

    /// `array_clear`: does not free anything.
    pub fn clear(&mut self) {
        self.size = 0;
    }

    /// `array_reserve`.
    ///
    /// # Safety
    /// The array must own its allocation (or be empty).
    pub unsafe fn reserve(&mut self, new_capacity: u32) {
        // SAFETY: forwarded.
        self.contents = unsafe {
            raw_reserve(
                self.contents as *mut u8,
                &mut self.capacity,
                Self::ELEM,
                new_capacity,
            )
        } as *mut T;
    }

    /// `array_delete`: frees the buffer and resets the header.
    ///
    /// # Safety
    /// The array must own its allocation (or be empty).
    pub unsafe fn delete(&mut self) {
        if !self.contents.is_null() {
            // SAFETY: forwarded.
            unsafe { ts_free(self.contents as *mut c_void) };
        }
        *self = Self::new();
    }

    /// `array_push`.
    ///
    /// # Safety
    /// As for `reserve`.
    pub unsafe fn push(&mut self, element: T) {
        // SAFETY: grow guarantees room for one more element.
        unsafe {
            self.contents = raw_grow(
                self.contents as *mut u8,
                self.size,
                &mut self.capacity,
                1,
                Self::ELEM,
            ) as *mut T;
            *self.contents.add(self.size as usize) = element;
        }
        self.size = self.size.wrapping_add(1);
    }

    /// `array_grow_by`: appends `count` zeroed elements.
    ///
    /// # Safety
    /// As for `reserve`.
    pub unsafe fn grow_by(&mut self, count: u32) {
        if count == 0 {
            return;
        }
        // SAFETY: grow guarantees room for `count` more elements.
        unsafe {
            self.contents = raw_grow(
                self.contents as *mut u8,
                self.size,
                &mut self.capacity,
                count,
                Self::ELEM,
            ) as *mut T;
            ptr::write_bytes(self.contents.add(self.size as usize), 0, count as usize);
        }
        self.size = self.size.wrapping_add(count);
    }

    /// `array_extend`.
    ///
    /// # Safety
    /// `other` points to `count` valid elements outside this array.
    pub unsafe fn extend(&mut self, count: u32, other: *const T) {
        let end = self.size;
        // SAFETY: forwarded.
        self.contents = unsafe {
            raw_splice(
                self.contents as *mut u8,
                &mut self.size,
                &mut self.capacity,
                Self::ELEM,
                end,
                0,
                count,
                other as *const u8,
            )
        } as *mut T;
    }

    /// `array_push_all`.
    ///
    /// # Safety
    /// `other` is a different, consistent array.
    pub unsafe fn push_all(&mut self, other: &Array<T>) {
        unsafe { self.extend(other.size, other.contents) }
    }

    /// `array_splice`.
    ///
    /// # Safety
    /// See `raw_splice`.
    pub unsafe fn splice(
        &mut self,
        index: u32,
        old_count: u32,
        new_count: u32,
        new_contents: *const T,
    ) {
        // SAFETY: forwarded.
        self.contents = unsafe {
            raw_splice(
                self.contents as *mut u8,
                &mut self.size,
                &mut self.capacity,
                Self::ELEM,
                index,
                old_count,
                new_count,
                new_contents as *const u8,
            )
        } as *mut T;
    }

    /// `array_insert`.
    ///
    /// # Safety
    /// As for `splice`.
    pub unsafe fn insert(&mut self, index: u32, element: T) {
        unsafe { self.splice(index, 0, 1, &element) }
    }

    /// `array_erase`.
    ///
    /// # Safety
    /// The array must be consistent.
    pub unsafe fn erase(&mut self, index: u32) {
        // SAFETY: forwarded.
        unsafe { raw_erase(self.contents as *mut u8, &mut self.size, Self::ELEM, index) }
    }

    /// `array_pop`: the C does not check for an empty array; neither does this (wrapping).
    ///
    /// # Safety
    /// The array must be non-empty.
    pub unsafe fn pop(&mut self) -> T {
        self.size = self.size.wrapping_sub(1);
        // SAFETY: caller guarantees size was > 0.
        unsafe { *self.contents.add(self.size as usize) }
    }

    /// `array_assign`.
    ///
    /// # Safety
    /// `other` is a different, consistent array.
    pub unsafe fn assign(&mut self, other: &Array<T>) {
        // SAFETY: forwarded.
        self.contents = unsafe {
            raw_assign(
                self.contents as *mut u8,
                &mut self.size,
                &mut self.capacity,
                other.contents as *const u8,
                other.size,
                Self::ELEM,
            )
        } as *mut T;
    }

    /// `array_swap`.
    pub fn swap(&mut self, other: &mut Array<T>) {
        core::mem::swap(self, other);
    }

    /// `_array__search_sorted`, specialised to a start of 0: returns `(index, exists)`.
    ///
    /// `compare(existing, needle)` has `memcmp`-like sign semantics.
    ///
    /// # Safety
    /// The array must be consistent.
    pub unsafe fn search_sorted_with<N>(
        &self,
        compare: impl Fn(&T, N) -> i32,
        needle: N,
    ) -> (u32, bool)
    where
        N: Copy,
    {
        let mut index = 0u32;
        let mut size = self.size;
        if size == 0 {
            return (0, false);
        }
        while size > 1 {
            let half_size = size / 2;
            let mid_index = index + half_size;
            // SAFETY: mid_index < self.size.
            let comparison = compare(unsafe { &*self.contents.add(mid_index as usize) }, needle);
            if comparison <= 0 {
                index = mid_index;
            }
            size -= half_size;
        }
        // SAFETY: index < self.size.
        let comparison = compare(unsafe { &*self.contents.add(index as usize) }, needle);
        if comparison == 0 {
            return (index, true);
        } else if comparison < 0 {
            index += 1;
        }
        (index, false)
    }

    /// `array_search_sorted_by`: compares an integer field of each element with `needle` using the
    /// C's `(int)existing - (int)needle`.
    ///
    /// # Safety
    /// The array must be consistent.
    pub unsafe fn search_sorted_by(&self, field: impl Fn(&T) -> u32, needle: u32) -> (u32, bool) {
        // SAFETY: forwarded.
        unsafe {
            self.search_sorted_with(|a, n: u32| (field(a) as i32).wrapping_sub(n as i32), needle)
        }
    }
}

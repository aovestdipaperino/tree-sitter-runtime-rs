//! Port of `src/lexer.c` / `src/lexer.h`.
//!
//! `Lexer` keeps the C layout because `TSParser` (still C) embeds one and reads its fields. The
//! `TSLexer` vtable that scanners and generated lex functions call through is filled with the
//! `extern "C"` functions below. `TSLexer.log` is variadic, so it is a small C function
//! (`c/lexer_log.c`) that calls back into [`ts_lexer_log_emit`].
use crate::alloc::ts_realloc;
use crate::api::{
    TSInput, TSInputEncodingUTF8, TSInputEncodingUTF16BE, TSInputEncodingUTF16LE, TSLogType,
    TSLogTypeLex, TSLogger, TSPoint, TSRange,
};
use crate::length::{LENGTH_UNDEFINED, Length, length_is_undefined};
use crate::parser_abi::{TREE_SITTER_SERIALIZATION_BUFFER_SIZE, TSLexer};
use crate::unicode::{TS_DECODE_ERROR, decode_utf8, decode_utf16_be, decode_utf16_le};
use core::ffi::{c_char, c_void};
use core::ptr;

const BYTE_ORDER_MARK: i32 = 0xFEFF;

static DEFAULT_RANGE: TSRange = TSRange {
    start_point: TSPoint { row: 0, column: 0 },
    end_point: TSPoint {
        row: u32::MAX,
        column: u32::MAX,
    },
    start_byte: 0,
    end_byte: u32::MAX,
};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ColumnData {
    pub value: u32,
    pub valid: bool,
}

#[repr(C)]
pub struct Lexer {
    pub data: TSLexer,
    pub current_position: Length,
    pub token_start_position: Length,
    pub token_end_position: Length,

    pub included_ranges: *mut TSRange,
    pub chunk: *const c_char,
    pub input: TSInput,
    pub logger: TSLogger,

    pub included_range_count: u32,
    pub current_included_range_index: u32,
    pub chunk_start: u32,
    pub chunk_size: u32,
    pub lookahead_size: u32,
    pub did_get_column: bool,
    pub column_data: ColumnData,

    pub debug_buffer: [c_char; TREE_SITTER_SERIALIZATION_BUFFER_SIZE],
}

unsafe extern "C" {
    /// The variadic `TSLexer.log`, defined in `c/lexer_log.c`.
    fn ts_lexer__log(this: *const TSLexer, fmt: *const c_char, ...);
}

/// Called by `ts_lexer__log` with the formatted message.
///
/// # Safety
/// `this` must point at the `data` field of a live `Lexer`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_lexer_log_emit(this: *const TSLexer, buffer: *const c_char) {
    // SAFETY: `data` is the first field of `Lexer` (repr(C)), as the C cast relies on.
    let lexer = unsafe { &mut *(this as *mut Lexer) };
    if let Some(log) = lexer.logger.log {
        // SAFETY: the logger contract is the user's.
        unsafe { log(lexer.logger.payload, TSLogTypeLex as TSLogType, buffer) };
    }
}

impl Lexer {
    fn set_column_data(&mut self, val: u32) {
        self.column_data.valid = true;
        self.column_data.value = val;
    }

    fn increment_column_data(&mut self) {
        if self.column_data.valid {
            self.column_data.value = self.column_data.value.wrapping_add(1);
        }
    }

    fn invalidate_column_data(&mut self) {
        self.column_data.valid = false;
        self.column_data.value = 0;
    }

    fn at_eof(&self) -> bool {
        self.current_included_range_index == self.included_range_count
    }

    fn clear_chunk(&mut self) {
        self.chunk = ptr::null();
        self.chunk_size = 0;
        self.chunk_start = 0;
    }

    /// `LOG(message, character)`: `message character:'c'` for printable ASCII, else the number.
    fn log_character(&mut self, message: &str, character: i32) {
        let Some(log) = self.logger.log else { return };
        let text = if (32..127).contains(&character) {
            format!("{message} character:'{}'", character as u8 as char)
        } else {
            format!("{message} character:{character}")
        };
        let bytes = text.as_bytes();
        let n = bytes.len().min(TREE_SITTER_SERIALIZATION_BUFFER_SIZE - 1);
        for (dst, &b) in self.debug_buffer.iter_mut().zip(&bytes[..n]) {
            *dst = b as c_char;
        }
        self.debug_buffer[n] = 0;
        // SAFETY: the logger contract is the user's.
        unsafe {
            log(
                self.logger.payload,
                TSLogTypeLex as TSLogType,
                self.debug_buffer.as_ptr(),
            )
        };
    }

    /// Calls the input callback for a new chunk at the current position.
    unsafe fn get_chunk(&mut self) {
        self.chunk_start = self.current_position.bytes;
        // SAFETY: the input callback is the user's; `read` is required by the API.
        self.chunk = unsafe {
            (self.input.read.unwrap_unchecked())(
                self.input.payload,
                self.current_position.bytes,
                self.current_position.extent,
                &mut self.chunk_size,
            )
        };
        if self.chunk_size == 0 {
            self.current_included_range_index = self.included_range_count;
            self.chunk = ptr::null();
        }
    }

    unsafe fn decode(&self, chunk: *const u8, size: u32, code_point: &mut i32) -> u32 {
        let slice = if size == 0 || chunk.is_null() {
            &[][..]
        } else {
            // SAFETY: the chunk holds `size` readable bytes (it came from the input callback).
            unsafe { core::slice::from_raw_parts(chunk, size as usize) }
        };
        let (used, cp) = match self.input.encoding {
            e if e == TSInputEncodingUTF8 => decode_utf8(slice),
            e if e == TSInputEncodingUTF16LE => decode_utf16_le(slice),
            e if e == TSInputEncodingUTF16BE => decode_utf16_be(slice),
            _ => {
                // SAFETY: custom decoder supplied by the user.
                let used =
                    unsafe { (self.input.decode.unwrap_unchecked())(chunk, size, code_point) };
                return used;
            }
        };
        *code_point = cp;
        used
    }

    /// Decodes the next character of the current chunk.
    unsafe fn get_lookahead(&mut self) {
        let position_in_chunk = self.current_position.bytes.wrapping_sub(self.chunk_start);
        let mut size = self.chunk_size.wrapping_sub(position_in_chunk);

        if size == 0 {
            self.lookahead_size = 1;
            self.data.lookahead = 0;
            return;
        }

        let mut chunk = (self.chunk as *const u8).wrapping_add(position_in_chunk as usize);
        let mut cp = self.data.lookahead;
        // SAFETY: see `decode`.
        self.lookahead_size = unsafe { self.decode(chunk, size, &mut cp) };
        self.data.lookahead = cp;

        // A chunk that ended in the middle of a multi-byte character: try again with a fresh chunk.
        if self.data.lookahead == TS_DECODE_ERROR && size < 4 {
            // SAFETY: as above.
            unsafe { self.get_chunk() };
            chunk = self.chunk as *const u8;
            size = self.chunk_size;
            // SAFETY: as above.
            self.lookahead_size = unsafe { self.decode(chunk, size, &mut cp) };
            self.data.lookahead = cp;
        }

        if self.data.lookahead == TS_DECODE_ERROR {
            self.lookahead_size = 1;
        }
    }

    unsafe fn goto(&mut self, position: Length) {
        if position.bytes != self.current_position.bytes {
            self.invalidate_column_data();
        }

        self.current_position = position;

        // Move to the first valid position at or after the given position.
        let mut found_included_range = false;
        for i in 0..self.included_range_count {
            // SAFETY: i < included_range_count.
            let included_range = unsafe { &*self.included_ranges.add(i as usize) };
            if included_range.end_byte > self.current_position.bytes
                && included_range.end_byte > included_range.start_byte
            {
                if included_range.start_byte >= self.current_position.bytes {
                    self.current_position = Length {
                        bytes: included_range.start_byte,
                        extent: included_range.start_point,
                    };
                }
                self.current_included_range_index = i;
                found_included_range = true;
                break;
            }
        }

        if found_included_range {
            // A position outside the current chunk invalidates it.
            if !self.chunk.is_null()
                && (self.current_position.bytes < self.chunk_start
                    || self.current_position.bytes
                        >= self.chunk_start.wrapping_add(self.chunk_size))
            {
                self.clear_chunk();
            }
            self.lookahead_size = 0;
            self.data.lookahead = 0;
        } else {
            // Beyond all included ranges: the EOF state.
            self.current_included_range_index = self.included_range_count;
            // SAFETY: the range count is never zero.
            let last = unsafe {
                &*self
                    .included_ranges
                    .add(self.included_range_count as usize - 1)
            };
            self.current_position = Length {
                bytes: last.end_byte,
                extent: last.end_point,
            };
            self.clear_chunk();
            self.lookahead_size = 1;
            self.data.lookahead = 0;
        }
    }

    /// Actually advances the lexer; logs nothing.
    unsafe fn do_advance(&mut self, skip: bool) {
        if self.lookahead_size != 0 {
            if self.data.lookahead == '\n' as i32 {
                self.current_position.extent.row = self.current_position.extent.row.wrapping_add(1);
                self.current_position.extent.column = 0;
                self.set_column_data(0);
            } else {
                let is_bom =
                    self.current_position.bytes == 0 && self.data.lookahead == BYTE_ORDER_MARK;
                if !is_bom {
                    self.increment_column_data();
                }
                self.current_position.extent.column = self
                    .current_position
                    .extent
                    .column
                    .wrapping_add(self.lookahead_size);
            }
            self.current_position.bytes = self
                .current_position
                .bytes
                .wrapping_add(self.lookahead_size);
        }

        // SAFETY: the index is < count whenever the lexer is not at EOF, and `advance` returns early at EOF.
        let mut current_range: *const TSRange = unsafe {
            self.included_ranges
                .add(self.current_included_range_index as usize)
        };
        // SAFETY: `current_range` is only dereferenced while it points into the ranges array.
        unsafe {
            while self.current_position.bytes >= (*current_range).end_byte
                || (*current_range).end_byte == (*current_range).start_byte
            {
                if self.current_included_range_index < self.included_range_count {
                    self.current_included_range_index += 1;
                }
                if self.current_included_range_index < self.included_range_count {
                    current_range = current_range.add(1);
                    self.current_position = Length {
                        bytes: (*current_range).start_byte,
                        extent: (*current_range).start_point,
                    };
                } else {
                    current_range = ptr::null();
                    break;
                }
            }
        }

        if skip {
            self.token_start_position = self.current_position;
        }

        if !current_range.is_null() {
            if self.current_position.bytes < self.chunk_start
                || self.current_position.bytes >= self.chunk_start.wrapping_add(self.chunk_size)
            {
                // SAFETY: input callback.
                unsafe { self.get_chunk() };
            }
            // SAFETY: chunk established above.
            unsafe { self.get_lookahead() };
        } else {
            self.clear_chunk();
            self.data.lookahead = 0;
            self.lookahead_size = 1;
        }
    }

    unsafe fn mark_end(&mut self) {
        if !self.at_eof() {
            // If the lexer is right at the start of an included range, the token ends at the
            // *end* of the previous range instead.
            // SAFETY: index < count (not at EOF).
            let current = unsafe {
                &*self
                    .included_ranges
                    .add(self.current_included_range_index as usize)
            };
            if self.current_included_range_index > 0
                && self.current_position.bytes == current.start_byte
            {
                // SAFETY: index > 0.
                let previous = unsafe {
                    &*self
                        .included_ranges
                        .add(self.current_included_range_index as usize - 1)
                };
                self.token_end_position = Length {
                    bytes: previous.end_byte,
                    extent: previous.end_point,
                };
                return;
            }
        }
        self.token_end_position = self.current_position;
    }
}

// ---- The TSLexer vtable ----

unsafe extern "C" fn lexer_eof(this: *const TSLexer) -> bool {
    // SAFETY: `data` is the first field of `Lexer`.
    unsafe { (*(this as *const Lexer)).at_eof() }
}

unsafe extern "C" fn lexer_advance(this: *mut TSLexer, skip: bool) {
    // SAFETY: as above.
    let lexer = unsafe { &mut *(this as *mut Lexer) };
    if lexer.chunk.is_null() {
        return;
    }
    let c = lexer.data.lookahead;
    lexer.log_character(if skip { "skip" } else { "consume" }, c);
    // SAFETY: chunk is set.
    unsafe { lexer.do_advance(skip) };
}

unsafe extern "C" fn lexer_mark_end(this: *mut TSLexer) {
    // SAFETY: as above.
    unsafe { (*(this as *mut Lexer)).mark_end() };
}

unsafe extern "C" fn lexer_get_column(this: *mut TSLexer) -> u32 {
    // SAFETY: as above.
    let lexer = unsafe { &mut *(this as *mut Lexer) };
    lexer.did_get_column = true;

    if !lexer.column_data.valid {
        let goal_byte = lexer.current_position.bytes;
        // Back up to the beginning of the line.
        let start_of_col = Length {
            bytes: lexer
                .current_position
                .bytes
                .wrapping_sub(lexer.current_position.extent.column),
            extent: TSPoint {
                row: lexer.current_position.extent.row,
                column: 0,
            },
        };
        // SAFETY: lexer methods need a live input, as in the C.
        unsafe {
            lexer.goto(start_of_col);
            lexer.set_column_data(0);
            lexer.get_chunk();

            if !lexer.at_eof() {
                lexer.get_lookahead();
                // Advance to the recorded position.
                while lexer.current_position.bytes < goal_byte
                    && !lexer.at_eof()
                    && !lexer.chunk.is_null()
                {
                    lexer.do_advance(false);
                    if lexer.at_eof() {
                        break;
                    }
                }
            }
        }
    }
    lexer.column_data.value
}

unsafe extern "C" fn lexer_is_at_included_range_start(this: *const TSLexer) -> bool {
    // SAFETY: as above.
    let lexer = unsafe { &*(this as *const Lexer) };
    if lexer.current_included_range_index < lexer.included_range_count {
        // SAFETY: index < count.
        let current = unsafe {
            &*lexer
                .included_ranges
                .add(lexer.current_included_range_index as usize)
        };
        lexer.current_position.bytes == current.start_byte
    } else {
        false
    }
}

// ---- Public (crate-internal C ABI) functions ----

/// # Safety
/// `this` points to writable `Lexer` storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_lexer_init(this: *mut Lexer) {
    // Unspecified fields are zero in the C compound literal.
    let lexer = Lexer {
        data: TSLexer {
            lookahead: 0,
            result_symbol: 0,
            advance: lexer_advance,
            mark_end: lexer_mark_end,
            get_column: lexer_get_column,
            is_at_included_range_start: lexer_is_at_included_range_start,
            eof: lexer_eof,
            log: Some(ts_lexer__log),
        },
        current_position: Length {
            bytes: 0,
            extent: TSPoint { row: 0, column: 0 },
        },
        token_start_position: Length {
            bytes: 0,
            extent: TSPoint { row: 0, column: 0 },
        },
        token_end_position: Length {
            bytes: 0,
            extent: TSPoint { row: 0, column: 0 },
        },
        included_ranges: ptr::null_mut(),
        chunk: ptr::null(),
        input: TSInput {
            payload: ptr::null_mut(),
            read: None,
            encoding: 0,
            decode: None,
        },
        logger: TSLogger {
            payload: ptr::null_mut(),
            log: None,
        },
        included_range_count: 0,
        current_included_range_index: 0,
        chunk_start: 0,
        chunk_size: 0,
        lookahead_size: 0,
        did_get_column: false,
        column_data: ColumnData {
            value: 0,
            valid: false,
        },
        debug_buffer: [0; TREE_SITTER_SERIALIZATION_BUFFER_SIZE],
    };
    // SAFETY: the caller owns uninitialised storage.
    unsafe {
        ptr::write(this, lexer);
        ts_lexer_set_included_ranges(this, ptr::null(), 0);
    }
}

/// # Safety
/// `this` is an initialised lexer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_lexer_delete(this: *mut Lexer) {
    // SAFETY: forwarded.
    unsafe { crate::alloc::ts_free((*this).included_ranges as *mut c_void) };
}

/// # Safety
/// `this` is an initialised lexer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_lexer_set_input(this: *mut Lexer, input: TSInput) {
    // SAFETY: forwarded.
    unsafe {
        let lexer = &mut *this;
        lexer.input = input;
        lexer.clear_chunk();
        let position = lexer.current_position;
        lexer.goto(position);
    }
}

/// Moves the lexer to `position`; no work if it is already there.
///
/// # Safety
/// `this` is an initialised lexer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_lexer_reset(this: *mut Lexer, position: Length) {
    // SAFETY: forwarded.
    unsafe {
        let lexer = &mut *this;
        if position.bytes != lexer.current_position.bytes {
            lexer.goto(position);
        }
    }
}

/// # Safety
/// `this` is an initialised lexer with an input.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_lexer_start(this: *mut Lexer) {
    // SAFETY: forwarded.
    unsafe {
        let lexer = &mut *this;
        lexer.token_start_position = lexer.current_position;
        lexer.token_end_position = LENGTH_UNDEFINED;
        lexer.data.result_symbol = 0;
        lexer.did_get_column = false;
        if !lexer.at_eof() {
            if lexer.chunk_size == 0 {
                lexer.get_chunk();
            }
            if lexer.lookahead_size == 0 {
                lexer.get_lookahead();
            }
            if lexer.current_position.bytes == 0 {
                if lexer.data.lookahead == BYTE_ORDER_MARK {
                    lexer_advance(&mut lexer.data, true);
                }
                lexer.set_column_data(0);
            }
        }
    }
}

/// # Safety
/// `this` is a started lexer; `lookahead_end_byte` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_lexer_finish(this: *mut Lexer, lookahead_end_byte: *mut u32) {
    // SAFETY: forwarded.
    unsafe {
        let lexer = &mut *this;
        if length_is_undefined(lexer.token_end_position) {
            lexer.mark_end();
        }

        // A token that ended at an included range boundary had its end reset to the end of the
        // preceding range; reset the start to match.
        if lexer.token_end_position.bytes < lexer.token_start_position.bytes {
            lexer.token_start_position = lexer.token_end_position;
        }

        let mut current_lookahead_end_byte = lexer.current_position.bytes.wrapping_add(1);

        // The decoder may have looked at the byte after an invalid character.
        if lexer.data.lookahead == TS_DECODE_ERROR {
            current_lookahead_end_byte = current_lookahead_end_byte.wrapping_add(4);
        }

        if current_lookahead_end_byte > *lookahead_end_byte {
            *lookahead_end_byte = current_lookahead_end_byte;
        }
    }
}

/// # Safety
/// `this` is an initialised lexer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_lexer_mark_end(this: *mut Lexer) {
    // SAFETY: forwarded.
    unsafe { (*this).mark_end() }
}

/// # Safety
/// `ranges` is null or points to `count` ranges; `this` is an initialised lexer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_lexer_set_included_ranges(
    this: *mut Lexer,
    mut ranges: *const TSRange,
    mut count: u32,
) -> bool {
    if count == 0 || ranges.is_null() {
        ranges = &DEFAULT_RANGE;
        count = 1;
    } else {
        let mut previous_byte = 0u32;
        for i in 0..count {
            // SAFETY: i < count.
            let range = unsafe { &*ranges.add(i as usize) };
            if range.start_byte < previous_byte || range.end_byte < range.start_byte {
                return false;
            }
            previous_byte = range.end_byte;
        }
    }

    let size = count as usize * size_of::<TSRange>();
    // SAFETY: forwarded; the new buffer holds `count` ranges.
    unsafe {
        let lexer = &mut *this;
        lexer.included_ranges =
            ts_realloc(lexer.included_ranges as *mut c_void, size) as *mut TSRange;
        ptr::copy_nonoverlapping(ranges, lexer.included_ranges, count as usize);
        lexer.included_range_count = count;
        let position = lexer.current_position;
        lexer.goto(position);
    }
    true
}

/// # Safety
/// `this` is an initialised lexer; `count` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ts_lexer_included_ranges(
    this: *const Lexer,
    count: *mut u32,
) -> *mut TSRange {
    // SAFETY: forwarded.
    unsafe {
        *count = (*this).included_range_count;
        (*this).included_ranges
    }
}

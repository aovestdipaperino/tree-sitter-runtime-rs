//! Port of `src/unicode.h`: the decoders the lexer uses (`ts_decode_utf8`, `ts_decode_utf16_le`,
//! `ts_decode_utf16_be`), with the ICU `U8_NEXT` / `U16_NEXT` macros they expand to.
//!
//! Each returns `(bytes_consumed, code_point)`; an ill-formed sequence gives [`TS_DECODE_ERROR`].
//! The bytes consumed on an error are exactly what the C macro leaves in `i`, including the
//! partial advances inside a multi-byte sequence.
//!
//! One quirk of the C is reproduced on purpose (ADR-004): in `U16_NEXT_BE` the trail unit is read
//! in host byte order without `be16toh`, so on a little-endian host a big-endian surrogate pair
//! is only recognised when its bytes happen to look like a trail unit when swapped.

pub const TS_DECODE_ERROR: i32 = -1;

const LEAD3_T1_BITS: [u8; 16] = [
    0x20, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x10, 0x30, 0x30,
];
const LEAD4_T1_BITS: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 0x1E, 0x0F, 0x0F, 0x0F, 0, 0, 0, 0];

/// Decodes one UTF-8 code point from the start of `s` (`ts_decode_utf8`).
///
/// `s` must not be empty: the C reads `s[0]` unconditionally, and callers never pass length 0.
pub fn decode_utf8(s: &[u8]) -> (u32, i32) {
    let length = s.len();
    debug_assert!(length > 0, "ts_decode_utf8 is never called with length 0");
    if length == 0 {
        return (0, TS_DECODE_ERROR);
    }
    let mut i = 1usize;
    let mut c = i32::from(s[0]);
    if c & 0x80 == 0 {
        return (1, c);
    }
    // `ok` mirrors the C `&&` chain; `i` is advanced exactly where the C advances it.
    let ok = 'chain: {
        if i == length {
            break 'chain false;
        }
        if c >= 0xe0 {
            if c < 0xf0 {
                // U+0800..U+FFFF except surrogates.
                c &= 0xf;
                let t1 = s[i];
                if LEAD3_T1_BITS[c as usize] & (1u8 << (t1 >> 5)) == 0 {
                    break 'chain false;
                }
                c = (c << 6) | i32::from(t1 & 0x3f);
            } else {
                // U+10000..U+10FFFF.
                c -= 0xf0;
                if c > 4 {
                    break 'chain false;
                }
                let t1 = s[i];
                if LEAD4_T1_BITS[usize::from(t1 >> 4)] & (1u8 << c) == 0 {
                    break 'chain false;
                }
                c = (c << 6) | i32::from(t1 & 0x3f);
                i += 1;
                if i == length {
                    break 'chain false;
                }
                let t2 = s[i].wrapping_sub(0x80);
                if t2 > 0x3f {
                    break 'chain false;
                }
                c = (c << 6) | i32::from(t2);
            }
            // The valid second-to-last trail byte is consumed.
            i += 1;
            if i == length {
                break 'chain false;
            }
        } else {
            // U+0080..U+07FF.
            if c < 0xc2 {
                break 'chain false;
            }
            c &= 0x1f;
        }
        // Last trail byte.
        let t = s[i].wrapping_sub(0x80);
        if t > 0x3f {
            break 'chain false;
        }
        c = (c << 6) | i32::from(t);
        i += 1;
        true
    };
    (i as u32, if ok { c } else { TS_DECODE_ERROR })
}

const SURROGATE_OFFSET: i32 = (0xd800 << 10) + 0xdc00 - 0x10000;

#[inline]
fn is_lead(c: i32) -> bool {
    (c as u32) & 0xffff_fc00 == 0xd800
}

#[inline]
fn is_trail(c: i32) -> bool {
    (c as u32) & 0xffff_fc00 == 0xdc00
}

fn decode_utf16(s: &[u8], first: fn([u8; 2]) -> u16) -> (u32, i32) {
    let length = s.len();
    if length < 2 {
        return (length as u32, TS_DECODE_ERROR);
    }
    let units = length / 2;
    let mut c = i32::from(first([s[0], s[1]]));
    let mut i = 1usize;
    if is_lead(c) && i != units {
        // The C reads the second unit in host byte order for both LE and BE.
        let c2 = i32::from(u16::from_ne_bytes([s[2], s[3]]));
        if is_trail(c2) {
            i += 1;
            c = (c << 10).wrapping_add(c2).wrapping_sub(SURROGATE_OFFSET);
        }
    }
    ((i * 2) as u32, c)
}

/// `ts_decode_utf16_le`.
pub fn decode_utf16_le(s: &[u8]) -> (u32, i32) {
    decode_utf16(s, u16::from_le_bytes)
}

/// `ts_decode_utf16_be`, including the C's host-order read of the trail unit.
pub fn decode_utf16_be(s: &[u8]) -> (u32, i32) {
    decode_utf16(s, u16::from_be_bytes)
}

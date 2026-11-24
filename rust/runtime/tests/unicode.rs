//! The Rust decoders against the C header versions (`ts_decode_*`), compared on consumed bytes
//! and code point for every buffer length.
use tree_sitter_runtime_rs::unicode::*;

unsafe extern "C" {
    fn shim_decode_utf8(s: *const u8, n: u32, cp: *mut i32) -> u32;
    fn shim_decode_utf16_le(s: *const u8, n: u32, cp: *mut i32) -> u32;
    fn shim_decode_utf16_be(s: *const u8, n: u32, cp: *mut i32) -> u32;
}

type CFn = unsafe extern "C" fn(*const u8, u32, *mut i32) -> u32;

fn c(f: CFn, buf: &[u8]) -> (u32, i32) {
    let mut cp = i32::MIN;
    let used = unsafe { f(buf.as_ptr(), buf.len() as u32, &mut cp) };
    (used, cp)
}

/// Bytes that probe every boundary of the UTF-8 state machine.
const PROBE: [u8; 12] = [
    0x00, 0x41, 0x7f, 0x80, 0x8f, 0x90, 0x9f, 0xa0, 0xbf, 0xc0, 0xc2, 0xff,
];

#[test]
fn utf8_matches_c_exhaustively_on_short_sequences() {
    let mut n = 0u64;
    for b0 in 0..=255u8 {
        for b1 in 0..=255u8 {
            for &b2 in &PROBE {
                for &b3 in &PROBE {
                    let full = [b0, b1, b2, b3];
                    for len in 1..=4 {
                        let buf = &full[..len];
                        assert_eq!(decode_utf8(buf), c(shim_decode_utf8, buf), "{buf:02x?}");
                        n += 1;
                    }
                }
            }
        }
    }
    assert!(n > 10_000_000);
}

#[test]
fn utf8_every_scalar_value_round_trips_like_c() {
    for cp in (0..=0x10ffffu32).filter(|c| !(0xd800..0xe000).contains(c)) {
        let mut b = [0u8; 4];
        let s = char::from_u32(cp)
            .unwrap()
            .encode_utf8(&mut b)
            .as_bytes()
            .to_vec();
        let (used, got) = decode_utf8(&s);
        assert_eq!((used as usize, got), (s.len(), cp as i32));
        assert_eq!((used, got), c(shim_decode_utf8, &s));
    }
}

/// Units that matter to surrogate handling, as seen from both byte orders.
fn second_units() -> Vec<u16> {
    let mut v: Vec<u16> = (0xd800u32..0xe000).map(|u| u as u16).collect();
    v.extend((0u32..=0xffff).step_by(13).map(|u| u as u16));
    v.extend([
        0, 0x41, 0xd7ff, 0xe000, 0xffff, 0x00dc, 0x00df, 0x01dc, 0xdc00,
    ]);
    v
}

fn check_utf16(rust: fn(&[u8]) -> (u32, i32), cf: CFn, to_bytes: fn(u16) -> [u8; 2]) {
    let seconds = second_units();
    for first in 0..=0xffffu16 {
        let a = to_bytes(first);
        // Lengths 0..=1 units and an odd trailing byte.
        for buf in [&a[..1], &a[..]] {
            if buf.len() < 2 {
                assert_eq!(rust(buf), c(cf, buf));
            }
        }
        let lead_like = (0xd800..0xe000).contains(&first)
            || first & 0xff00 == 0xd800
            || first & 0x00ff == 0x00d8;
        let seconds: &[u16] = if lead_like {
            &seconds
        } else {
            &seconds[..40.min(seconds.len())]
        };
        for &second in seconds {
            let b = to_bytes(second);
            let full = [a[0], a[1], b[0], b[1]];
            for len in 2..=4 {
                let buf = &full[..len];
                assert_eq!(rust(buf), c(cf, buf), "{buf:02x?}");
            }
        }
    }
}

#[test]
fn utf16_le_matches_c() {
    check_utf16(decode_utf16_le, shim_decode_utf16_le, u16::to_le_bytes);
}

#[test]
fn utf16_be_matches_c_including_the_host_order_quirk() {
    check_utf16(decode_utf16_be, shim_decode_utf16_be, u16::to_be_bytes);
}

#[test]
fn utf16_be_quirk_is_real_on_little_endian_hosts() {
    // D83D DE00 is U+1F600 in UTF-16 BE; the C does not recognise the pair on a little-endian host.
    let bytes = [0xd8, 0x3d, 0xde, 0x00];
    let (used, cp) = decode_utf16_be(&bytes);
    if cfg!(target_endian = "little") {
        assert_eq!((used, cp), (2, 0xd83d));
    } else {
        assert_eq!((used, cp), (4, 0x1f600));
    }
    assert_eq!((used, cp), c(shim_decode_utf16_be, &bytes));
    assert_eq!(decode_utf16_le(&[0x3d, 0xd8, 0x00, 0xde]), (4, 0x1f600));
}

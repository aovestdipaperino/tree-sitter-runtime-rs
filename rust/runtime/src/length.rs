//! Port of `src/length.h`: a distance measured in bytes and as a point extent.
use crate::api::TSPoint;
use crate::point::{point_add, point_sub};

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Length {
    pub bytes: u32,
    pub extent: TSPoint,
}

pub const LENGTH_UNDEFINED: Length = Length {
    bytes: 0,
    extent: TSPoint { row: 0, column: 1 },
};
pub const LENGTH_MAX: Length = Length {
    bytes: u32::MAX,
    extent: TSPoint {
        row: u32::MAX,
        column: u32::MAX,
    },
};

#[inline]
pub fn length_is_undefined(length: Length) -> bool {
    length.bytes == 0 && length.extent.column != 0
}

#[inline]
pub fn length_min(a: Length, b: Length) -> Length {
    if a.bytes < b.bytes { a } else { b }
}

#[inline]
pub fn length_add(a: Length, b: Length) -> Length {
    Length {
        bytes: a.bytes.wrapping_add(b.bytes),
        extent: point_add(a.extent, b.extent),
    }
}

#[inline]
pub fn length_sub(a: Length, b: Length) -> Length {
    Length {
        bytes: a.bytes.saturating_sub(b.bytes),
        extent: point_sub(a.extent, b.extent),
    }
}

#[inline]
pub fn length_zero() -> Length {
    Length {
        bytes: 0,
        extent: TSPoint { row: 0, column: 0 },
    }
}

#[inline]
pub fn length_saturating_sub(a: Length, b: Length) -> Length {
    if a.bytes > b.bytes {
        length_sub(a, b)
    } else {
        length_zero()
    }
}

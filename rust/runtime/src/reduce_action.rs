//! Port of `src/reduce_action.h`.
use crate::api::TSSymbol;
use crate::array::Array;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReduceAction {
    pub count: u32,
    pub symbol: TSSymbol,
    pub dynamic_precedence: i32,
    pub production_id: u16,
}

pub type ReduceActionSet = Array<ReduceAction>;

/// `ts_reduce_action_set_add`: appends `new_action` unless one with the same symbol and count exists.
///
/// # Safety
/// `set` must be a consistent array owning its allocation.
pub unsafe fn ts_reduce_action_set_add(set: &mut ReduceActionSet, new_action: ReduceAction) {
    for i in 0..set.size {
        // SAFETY: i < size.
        let action = unsafe { *set.contents.add(i as usize) };
        if action.symbol == new_action.symbol && action.count == new_action.count {
            return;
        }
    }
    // SAFETY: forwarded contract.
    unsafe { set.push(new_action) };
}

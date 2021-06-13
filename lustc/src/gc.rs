//! Garbage collection

//! WARNING: Garbage collection will currently only work on Unix
//! platforms where the stack grows down.

use cranelift::prelude::*;
use cranelift_codegen::ir::function::Function;
use cranelift_codegen::isa::TargetIsa;
use lazy_static::lazy_static;

use std::sync::Mutex;

/// The number of bytes that have been allocated since the last gc run.
static mut ALLOC_AMOUNT: crate::Word = 0;

/// The amount of memory that can be allocated before we trigger a run
/// of the garbage collector. This is the same threshold suggested by
/// emacs lsp for increased emacs performance.
const GC_THRESHOLD: crate::Word = 0; // 100000000;

lazy_static! {
    /// A map between function ids and their stackmaps.
    static ref SM_REGISTRY: Mutex<Vec<(Vec<usize>, Vec<usize>)>> = Mutex::new(vec![]);
}

// Shim which will later do garbage collection. Has extremely crude
// heuristic for when to do allocation. Real garbage collection should
// update this.
pub extern "C" fn do_gc(amount: crate::Word) {
    unsafe {
        ALLOC_AMOUNT += amount;
    }
    // Trigger garbage collection if we're using over our gc threshold.
    // memory.
    if unsafe { true || ALLOC_AMOUNT > GC_THRESHOLD } {
        println!("GC!");
        backtrace::trace(|frame| {
            let sp = frame.sp();
            // When we compile functions we ask them to push
            // information to their stack about what functon they
            // are. This allows us to lookup information about what
            // live references they have on their stack here. The way
            // this is done is each function pushes
            // `0xba5eba11<function id>` to their stack when
            // called. In order to find the function id and perform
            // the lookup we search for 0xba5eba11 and then the next
            // word is the id.

            // Try to find the totem for 10 iterations. Experiments
            // suggest that Cranelift will put this in the first stack
            // location for the function so we really shouldn't be
            // looking for that long.
            for offset in 0..10 {
                // Want to move in increments of entire words instead
                // of bytes.
                let offset = offset * 8;
                let id: i64 = unsafe { *(sp.offset(-offset) as *const i64) };
                if id == 0xBA5EBA11 {
                    let id = unsafe { *(sp.offset(-offset + 8) as *const i64) };
                    let registry = SM_REGISTRY.lock().unwrap();
                    let (escaped, local) = &registry[id as usize];
                    for offset in local {
                        let offset = (*offset * 8) as isize;
                        let val = unsafe { *(sp.offset(offset) as *const i64) };
                        println!("val: {}", crate::Expr::from_immediate(val));
                    }
                    break;
                }
            }
            true // keep going to the next frame
        });
    }
}

/// Collects local values created during function compilation and then
/// builds stackmaps for those values.
pub struct LocalValueCollector {
    /// Values that hold escaped varaibles. These values will not be
    /// lust values but instead will be pointers to escaped ones.
    escaped: Vec<Value>,
    /// Reguar local values.
    local: Vec<Value>,
}

impl LocalValueCollector {
    pub fn new() -> Self {
        Self {
            escaped: vec![],
            local: vec![],
        }
    }

    /// Registers a local value that represents an escaped lust value.
    pub fn register_escaped(&mut self, val: Value) {
        self.escaped.push(val)
    }

    /// Registers a local value that represents a non-escaped lust value.
    pub fn register_local(&mut self, val: Value) {
        self.local.push(val)
    }

    /// Clears the collector for collection in another function.
    pub fn clear(&mut self) {
        self.escaped.clear();
        self.local.clear();
    }

    /// Gets the escaped and local stackmaps for this function.
    pub fn get_maps(self) -> (Vec<Value>, Vec<Value>) {
        (self.escaped, self.local)
    }

    pub fn show(&self) {
        println!("escaped: {:?}", self.escaped);
        println!("local: {:?}", self.local);
    }
}

pub fn root_offsets_from_values(
    args: &[Value],
    func: &Function,
    isa: &dyn TargetIsa,
) -> Vec<usize> {
    let loc = &func.locations;
    let mut live_ref_in_stack_slot = std::collections::HashSet::new();
    for val in args {
        if let Some(value_loc) = loc.get(*val) {
            match *value_loc {
                codegen::ir::ValueLoc::Stack(stack_slot) => {
                    live_ref_in_stack_slot.insert(stack_slot);
                }
                _ => {}
            }
        }
    }

    let stack = &func.stack_slots;
    let info = func.stack_slots.layout_info.unwrap();

    let word_size = isa.pointer_bytes() as usize;

    let mut vec = vec![];

    for (ss, ssd) in stack.iter() {
        if !live_ref_in_stack_slot.contains(&ss)
            || ssd.kind == codegen::ir::stackslot::StackSlotKind::OutgoingArg
        {
            continue;
        }

        debug_assert!(ssd.size as usize == word_size);
        let bytes_from_bottom = info.frame_size as i32 + ssd.offset.unwrap();
        let words_from_bottom = (bytes_from_bottom as usize) / word_size;
        vec.push(words_from_bottom);
    }

    vec
}

/// Registers a pair of stack maps with the stack map registry.
pub fn register_stackmaps(
    id: i64,
    func: &Function,
    isa: &dyn TargetIsa,
    maps: (Vec<Value>, Vec<Value>),
) {
    let offset_lists = (
        root_offsets_from_values(&maps.0, func, isa),
        root_offsets_from_values(&maps.1, func, isa),
    );
    let mut registry = SM_REGISTRY.lock().unwrap();
    // Resize so that we can fit the new number of values. Fill with
    // nonsense.
    registry.resize(id as usize + 1, (vec![], vec![]));
    registry[id as usize] = offset_lists;
}

#[cfg(test)]
mod tests {
    use crate::roundtrip_file;

    use super::*;

    #[test]
    fn sm_registry() {
        roundtrip_file("examples/fn.lisp").unwrap();
        let registry = SM_REGISTRY.lock().unwrap();
        assert_eq!(registry.len(), 3)
    }
}

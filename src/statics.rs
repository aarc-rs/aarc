use crate::hyaline::{Context, DeferredFn, Slot};
use std::cell::RefCell;
use std::ptr::null_mut;
use std::sync::{Arc, OnceLock, Weak};

const SLOTS_PER_NODE: usize = 64;

static CONTEXT: OnceLock<Context<SLOTS_PER_NODE>> = OnceLock::new();

fn get_context() -> &'static Context<SLOTS_PER_NODE> {
    CONTEXT.get_or_init(Context::default)
}

thread_local! {
    static SLOT_HANDLE: RefCell<SlotHandle> = RefCell::default();
}

fn get_slot() -> *mut Slot {
    SLOT_HANDLE.with_borrow_mut(|h| {
        if h.slot.is_null() {
            h.slot = get_context().reserve_slot();
        }
        h.slot
    })
}

pub(crate) fn begin_critical_section() {
    unsafe {
        (*get_slot()).activate();
    }
}

pub(crate) fn end_critical_section() {
    unsafe {
        (*get_slot()).deactivate();
    }
}

pub(crate) fn retire<T, const IS_STRONG: bool>(ptr: *const T) {
    get_context().retire(
        get_slot(),
        DeferredFn {
            ptr: ptr as *mut u8,
            f: Box::new(|p| unsafe {
                if IS_STRONG {
                    drop(Arc::from_raw(p as *const T));
                } else {
                    drop(Weak::from_raw(p as *const T));
                };
            }),
        },
    );
}

struct SlotHandle {
    slot: *mut Slot,
}

impl Default for SlotHandle {
    fn default() -> Self {
        Self { slot: null_mut() }
    }
}

impl Drop for SlotHandle {
    fn drop(&mut self) {
        if !self.slot.is_null() {
            get_context().leave_slot(self.slot);
        }
    }
}

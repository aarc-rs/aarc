use crate::hyaline::{Context, DeferredFn, Slot};
use std::cell::RefCell;
use std::ptr::null_mut;
use std::sync::{Arc, OnceLock, Weak};
use std::thread::AccessError;

const SLOTS_PER_NODE: usize = 64;

static CONTEXT: OnceLock<Context<SLOTS_PER_NODE>> = OnceLock::new();

fn get_context() -> &'static Context<SLOTS_PER_NODE> {
    CONTEXT.get_or_init(Context::default)
}

thread_local! {
    static SLOT_HANDLE: RefCell<SlotHandle> = RefCell::default();
}

fn get_slot() -> Result<*mut Slot, AccessError> {
    SLOT_HANDLE.try_with(|h| {
        let mut borrowed = h.borrow_mut();
        if borrowed.slot.is_null() {
            borrowed.slot = get_context().reserve_slot();
        }
        borrowed.slot
    })
}

pub(crate) fn acquire() {
    unsafe {
        (*get_slot().unwrap()).acquire();
    }
}

pub(crate) fn release() {
    unsafe {
        (*get_slot().unwrap()).release();
    }
}

pub(crate) fn retire<T, const IS_STRONG: bool>(ptr: *const T) {
    if let Ok(slot) = get_slot() {
        get_context().retire(
            slot,
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
    } else {
        // The TLS key is being destroyed. This path is only safe during cleanup.
        unsafe {
            if IS_STRONG {
                drop(Arc::from_raw(ptr));
            } else {
                drop(Weak::from_raw(ptr));
            };
        }
    }
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

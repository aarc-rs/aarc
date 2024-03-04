use crate::utils::unrolled_linked_list::UnrolledLinkedList;
use crate::utils::unsafe_arc::{UnsafeArc, UnsafeArcInner};
use std::cell::RefCell;
use std::mem;
use std::ops::DerefMut;
use std::ptr::null_mut;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize};

#[derive(Default)]
pub(crate) struct Context<const N: usize> {
    slots: UnrolledLinkedList<Slot, N>,
    slots_reserved: AtomicUsize,
}

impl<const N: usize> Context<N> {
    pub(crate) fn reserve_slot(&self) -> *mut Slot {
        self.slots_reserved.fetch_add(1, SeqCst);
        self.slots.try_for_each_with_append(|slot| {
            match slot
                .is_reserved
                .compare_exchange(false, true, SeqCst, SeqCst)
            {
                Ok(_) => Some((slot as *const Slot).cast_mut()),
                Err(_) => None,
            }
        })
    }
    pub(crate) fn leave_slot(&self, slot: *mut Slot) {
        unsafe {
            (*slot).is_reserved.store(false, SeqCst);
        }
        if self.slots_reserved.fetch_sub(1, SeqCst) == 1 {
            // The last participant to leave should try to clean up every slot.
            for slot in self.slots.iter(SeqCst) {
                match slot
                    .is_reserved
                    .compare_exchange(false, true, SeqCst, SeqCst)
                {
                    Ok(_) => {
                        drop(mem::take(slot.batch.borrow_mut().deref_mut()));
                        slot.detach_head();
                        slot.is_reserved.store(false, SeqCst);
                    }
                    Err(_) => break,
                }
            }
        }
    }
    pub(crate) fn retire(&self, slot: *mut Slot, deferred_fn: DeferredFn) {
        unsafe {
            let mut borrowed = (*slot).batch.borrow_mut();
            borrowed.push(deferred_fn);
            if borrowed.len() < self.slots_reserved.load(SeqCst) {
                return;
            }
            let batch = UnsafeArc::new(mem::take(borrowed.deref_mut()), 1);
            for s in self.slots.iter(SeqCst) {
                if (s as *const Slot == slot) || !s.is_active.load(SeqCst) {
                    continue;
                }
                let new = UnsafeArc::new(
                    CollectionNode {
                        _batch: batch.clone(),
                        next: None,
                    },
                    2,
                );
                let next = s.head.swap(new.ptr, SeqCst);
                if !next.is_null() {
                    (*new.ptr).item.next = Some(UnsafeArc::from_raw(next));
                }
            }
            // The borrow must be dropped before batch is dropped.
            drop(borrowed);
        }
    }
}

unsafe impl<const N: usize> Send for Context<N> {}

unsafe impl<const N: usize> Sync for Context<N> {}

struct CollectionNode {
    _batch: UnsafeArc<Vec<DeferredFn>>,
    next: Option<UnsafeArc<CollectionNode>>,
}

#[derive(Default)]
pub(crate) struct Slot {
    head: AtomicPtr<UnsafeArcInner<CollectionNode>>,
    batch: RefCell<Vec<DeferredFn>>,
    is_active: AtomicBool,
    is_reserved: AtomicBool,
}

impl Slot {
    pub(crate) fn activate(&self) {
        self.is_active.store(true, SeqCst);
    }
    pub(crate) fn deactivate(&self) {
        self.is_active.store(false, SeqCst);
        self.detach_head();
    }
    fn detach_head(&self) {
        unsafe {
            let ptr = self.head.swap(null_mut(), SeqCst);
            if !ptr.is_null() {
                drop(UnsafeArc::from_raw(ptr));
            }
        }
    }
}

pub(crate) struct DeferredFn {
    pub(crate) ptr: *mut u8,
    pub(crate) f: Box<dyn Fn(*mut u8)>,
}

impl Drop for DeferredFn {
    fn drop(&mut self) {
        (*self.f)(self.ptr);
    }
}

use crate::smr::drc::{Protect, ProtectPtr, Release, Retire};
use crate::utils::unrolled_linked_list::UnrolledLinkedList;
use crate::utils::unsafe_arc::UnsafeArc;
use std::cell::RefCell;
use std::collections::HashSet;
use std::mem;
use std::ops::DerefMut;
use std::ptr::null_mut;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::atomic::{AtomicBool, AtomicPtr};
use std::sync::OnceLock;

const SLOTS_PER_NODE: usize = 32;

/// The default memory reclamation strategy.
pub struct StandardReclaimer;

impl StandardReclaimer {
    /// # Safety
    /// TODO: write docs for this and make it pub
    #[allow(dead_code)]
    pub(crate) unsafe fn cleanup() {
        for slot in Self::get_all_slots().iter(SeqCst) {
            drop(slot.batch.take());
            slot.primary_list.detach_head();
            for snapshot_ptr in slot.snapshots.iter(SeqCst) {
                snapshot_ptr.conflicts.detach_head();
            }
        }
    }
    fn get_all_slots() -> &'static UnrolledLinkedList<Slot, SLOTS_PER_NODE> {
        static SLOTS: OnceLock<UnrolledLinkedList<Slot, SLOTS_PER_NODE>> = OnceLock::new();
        SLOTS.get_or_init(UnrolledLinkedList::default)
    }
    thread_local! {
        static SLOT_LOOKUP: RefCell<SlotHandle> = Default::default();
    }
    fn get_or_claim_slot() -> &'static Slot {
        Self::SLOT_LOOKUP.with_borrow_mut(|lookup| {
            if let Some(slot) = lookup.0 {
                slot
            } else {
                let claimed = Self::get_all_slots().try_for_each_with_append(|slot| {
                    slot.is_claimed
                        .compare_exchange(false, true, SeqCst, SeqCst)
                        .is_ok()
                });
                lookup.0 = Some(claimed);
                claimed
            }
        })
    }
}

impl Protect for StandardReclaimer {
    fn begin_critical_section() {
        Self::get_or_claim_slot()
            .is_in_critical_section
            .store(true, SeqCst);
    }

    fn end_critical_section() {
        let slot = Self::get_or_claim_slot();
        slot.is_in_critical_section.store(false, SeqCst);
        slot.primary_list.detach_head();
    }
}

impl ProtectPtr for StandardReclaimer {
    type ProtectionHandle = SnapshotPtr;
    fn protect_ptr(ptr: *mut u8) -> &'static SnapshotPtr {
        // TODO: don't search from the beginning every time
        Self::get_or_claim_slot()
            .snapshots
            .try_for_each_with_append(|s| {
                s.ptr
                    .compare_exchange(null_mut(), ptr, SeqCst, SeqCst)
                    .is_ok()
            })
    }
}

impl Retire for StandardReclaimer {
    fn retire(ptr: *mut u8, f: Box<dyn Fn()>) {
        let mut borrowed = Self::get_or_claim_slot().batch.borrow_mut();
        borrowed.functions.push(f);
        borrowed.ptrs.insert(ptr);
        if borrowed.functions.len() < borrowed.functions.capacity() {
            return;
        }
        let all_slots = Self::get_all_slots();
        let next_batch_size = all_slots.get_nodes_count() * SLOTS_PER_NODE;
        let batch = mem::replace(
            borrowed.deref_mut(),
            Batch {
                functions: Vec::with_capacity(next_batch_size),
                ptrs: HashSet::with_capacity(next_batch_size),
            },
        );
        // Drop the borrow before proceeding in case there is a recursive call to this function.
        drop(borrowed);
        let batch_arc = UnsafeArc::new(batch, 1);
        for slot in all_slots.iter(SeqCst) {
            if slot.is_in_critical_section.load(SeqCst) {
                // If a thread is in a critical section, it must be made aware of any retirements.
                // The snapshots will be checked when that thread exits the critical section.
                slot.primary_list.insert(batch_arc.clone(), Some(slot));
            } else {
                // Otherwise, the snapshots must be checked immediately.
                for snapshot_ptr in slot.snapshots.iter(SeqCst) {
                    let p = snapshot_ptr.ptr.load(SeqCst);
                    if !p.is_null() && batch_arc.ptrs.contains(&p) {
                        snapshot_ptr.conflicts.insert(batch_arc.clone(), None);
                    }
                }
            }
        }
    }
}

const SNAPSHOT_PTRS_PER_NODE: usize = 8;

#[derive(Default)]
struct Slot {
    primary_list: CollectionList,
    batch: RefCell<Batch>,
    snapshots: UnrolledLinkedList<SnapshotPtr, SNAPSHOT_PTRS_PER_NODE>,
    // TODO: snapshots could share entries if their pointers are equal
    // snapshots_by_addr_count: RefCell<HashMap<usize, usize>>,
    is_in_critical_section: AtomicBool,
    is_claimed: AtomicBool,
}

unsafe impl Send for Slot {}
unsafe impl Sync for Slot {}

#[derive(Default)]
pub struct SnapshotPtr {
    ptr: AtomicPtr<u8>,
    conflicts: CollectionList,
}

impl Release for SnapshotPtr {
    fn release(&self) {
        self.ptr.store(null_mut(), SeqCst);
        self.conflicts.detach_head();
    }
}

#[derive(Default)]
struct CollectionList {
    head: AtomicPtr<CollectionNode>,
}

impl CollectionList {
    fn insert(&self, batch: UnsafeArc<Batch>, check_on_drop: Option<&'static Slot>) {
        let mut new = UnsafeArc::new(
            CollectionNode {
                batch,
                next: None,
                check_on_drop,
            },
            2,
        );
        let next = self.head.swap(UnsafeArc::as_ptr(&new), SeqCst);
        if !next.is_null() {
            unsafe {
                new.next = Some(UnsafeArc::from_raw(next));
            }
        }
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

struct CollectionNode {
    batch: UnsafeArc<Batch>,
    next: Option<UnsafeArc<CollectionNode>>,
    check_on_drop: Option<&'static Slot>,
}

impl Drop for CollectionNode {
    fn drop(&mut self) {
        if let Some(slot) = self.check_on_drop {
            for snapshot_ptr in slot.snapshots.iter(SeqCst) {
                let ptr = snapshot_ptr.ptr.load(SeqCst);
                if !ptr.is_null() && self.batch.ptrs.contains(&ptr) {
                    // TODO: figure out how to do this by moving instead of cloning (RefCell?)
                    snapshot_ptr.conflicts.insert(self.batch.clone(), None);
                }
            }
        }
    }
}

#[derive(Default)]
struct Batch {
    functions: Vec<Box<dyn Fn()>>,
    ptrs: HashSet<*mut u8>,
}

impl Drop for Batch {
    fn drop(&mut self) {
        for f in self.functions.iter() {
            (**f)();
        }
    }
}

#[derive(Default)]
struct SlotHandle(Option<&'static Slot>);

impl Drop for SlotHandle {
    fn drop(&mut self) {
        if let Some(slot) = self.0 {
            slot.is_claimed.store(false, SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::smr::drc::{Protect, ProtectPtr, Release, Retire};
    use crate::smr::standard_reclaimer::{Batch, StandardReclaimer};
    use std::alloc::{dealloc, Layout};
    use std::cell::Cell;
    use std::collections::HashSet;
    use std::ptr::null_mut;
    use std::sync::atomic::Ordering::SeqCst;

    fn with_flag<F: Fn(&'static mut Cell<bool>)>(f: F) {
        let flag: &'static mut Cell<bool> = Box::leak(Box::new(Cell::new(false)));
        let flag_ptr = flag as *mut Cell<bool> as *mut u8;
        f(flag);
        unsafe {
            dealloc(flag_ptr, Layout::new::<Cell<bool>>());
        }
    }

    #[test]
    fn test_protect_and_retire() {
        with_flag(|flag| {
            let dummy_ptr = (flag as *const Cell<bool>) as *mut u8;

            StandardReclaimer::begin_critical_section();
            let slot = StandardReclaimer::get_or_claim_slot();
            assert!(slot.is_in_critical_section.load(SeqCst));

            StandardReclaimer::retire(dummy_ptr, Box::new(|| flag.set(true)));
            assert!(!flag.get());

            StandardReclaimer::end_critical_section();
            assert!(!slot.is_in_critical_section.load(SeqCst));

            drop(slot.batch.take());
            assert!(flag.get());
        });
    }

    #[test]
    fn test_protect_ptr_and_release() {
        with_flag(|flag| {
            let dummy_ptr = (flag as *const Cell<bool>) as *mut u8;

            StandardReclaimer::get_or_claim_slot().batch.replace(Batch {
                functions: Vec::with_capacity(1),
                ptrs: HashSet::with_capacity(1),
            });

            let handle = StandardReclaimer::protect_ptr(dummy_ptr);
            assert_eq!(handle.ptr.load(SeqCst), dummy_ptr);

            StandardReclaimer::retire(dummy_ptr, Box::new(|| flag.set(true)));
            assert!(!flag.get());

            handle.release();
            assert_eq!(handle.ptr.load(SeqCst), null_mut());
            assert!(flag.get());
        });
    }
}

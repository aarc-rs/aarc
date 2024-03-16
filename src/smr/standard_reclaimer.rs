use crate::smr::drc::{Protect, ProtectPtr, ProvideGlobal, Release, Retire};
use crate::utils::unrolled_linked_list::UnrolledLinkedList;
use crate::utils::unsafe_arc::UnsafeArc;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::mem;
use std::ops::DerefMut;
use std::ptr::{null, null_mut};
use std::sync::atomic::Ordering::SeqCst;
use std::sync::atomic::{AtomicBool, AtomicPtr};
use std::sync::OnceLock;

/// Shortcut for declaring a wrapper type for a [`StandardReclaimer`].
///
/// See: [newtype idiom](https://doc.rust-lang.org/rust-by-example/generics/new_types.html).
/// This provides safety guarantees at compile time in situations where multiple reclaimer
/// instances are to be used. For example, an atomic associated with one reclaimer will not be able
/// to store a pointer to an allocation associated with a different reclaimer.
///
/// The declared type has a `new` method that may only be called once, as a guardrail.
///
/// # Examples:
/// ```should_panic
/// use aarc::standard_reclaimer_newtype;
///
/// standard_reclaimer_newtype!(MyReclaimer);
/// let r1 = MyReclaimer::new();
/// let r2 = MyReclaimer::new(); // not allowed
/// ```
///
/// The macro expansion does something similar to the following:
/// ```text
/// struct MyReclaimer(StandardReclaimer);
///
/// impl Trait1 for ExampleName {
///     fn behavior1(&self) {
///         self.0.behavior1();
///     }
/// }
///
/// impl Trait2 for ExampleName {
///     ...
/// }
///
/// ...
/// ```
/// where `MyReclaimer` is the `$name` of the new type provided by the user, and every trait that
/// is implemented for `StandardReclaimer` will also be implemented for `MyReclaimer`.
#[macro_export]
macro_rules! standard_reclaimer_newtype {
    ($name: ident) => {
        pub struct $name($crate::smr::standard_reclaimer::StandardReclaimer);

        impl $name {
            pub fn new() -> Self {
                static ALREADY_CALLED: std::sync::atomic::AtomicBool =
                    std::sync::atomic::AtomicBool::new(false);
                assert!(!ALREADY_CALLED.swap(true, std::sync::atomic::Ordering::SeqCst));
                Self($crate::smr::standard_reclaimer::StandardReclaimer::new())
            }
        }

        impl $crate::smr::drc::Protect for $name {
            fn begin_critical_section(&self) {
                self.0.begin_critical_section();
            }
            fn end_critical_section(&self) {
                self.0.end_critical_section();
            }
        }

        impl $crate::smr::drc::ProtectPtr for $name {
            type ProtectionHandle = $crate::smr::standard_reclaimer::SnapshotPtr;

            fn protect_ptr(&self, ptr: *mut u8) -> &Self::ProtectionHandle {
                self.0.protect_ptr(ptr)
            }
        }
        impl $crate::smr::drc::Retire for $name {
            fn retire(&self, ptr: *mut u8, f: Box<dyn Fn(*mut u8)>) {
                self.0.retire(ptr, f);
            }
            fn drop_flag(&self) -> &std::cell::Cell<bool> {
                self.0.drop_flag()
            }
        }

        unsafe impl Send for $name {}
        unsafe impl Sync for $name {}
    };
}

const SLOTS_PER_NODE: usize = 32;

/// The default memory reclamation strategy.
#[derive(Default)]
pub struct StandardReclaimer {
    slots: UnrolledLinkedList<Slot, SLOTS_PER_NODE>,
    is_dropped: UnsafeArc<Cell<bool>>,
}

impl StandardReclaimer {
    /// Creates a new `StandardReclaimer`.
    ///
    /// # Safety:
    /// If more than one `StandardReclaimer` will be used, new wrapper types should be created
    /// using the [`standard_reclaimer_newtype`] macro. Otherwise, it will be up to the user to
    /// ensure that objects associated with different reclaimer instances do not intermingle.
    pub fn new() -> Self {
        Self::default()
    }
    thread_local! {
        static SLOT_LOOKUP: RefCell<SlotLookup> = Default::default();
    }
    fn get_or_claim_slot(&self) -> &Slot {
        Self::SLOT_LOOKUP.with_borrow_mut(|lookup| unsafe {
            let (slot, _) = lookup.0.entry(self as *const Self).or_insert_with(|| {
                let slot = self.slots.try_for_each_with_append(|slot| {
                    slot.is_claimed
                        .compare_exchange(false, true, SeqCst, SeqCst)
                        .is_ok()
                });
                (slot, self.is_dropped.clone())
            });
            &**slot
        })
    }
}

impl Drop for StandardReclaimer {
    fn drop(&mut self) {
        self.is_dropped.set(true);
        // Clean up every slot.
        for slot in self.slots.iter(SeqCst) {
            drop(slot.batch.take());
            slot.primary_list.detach_head();
            for snapshot_ptr in slot.snapshots.iter(SeqCst) {
                snapshot_ptr.conflicts.detach_head();
            }
        }
        // Remove the entry in the thread-local lookup
        Self::SLOT_LOOKUP.with_borrow_mut(|lookup| {
            lookup.0.remove(&(self as *const Self));
        });
    }
}

impl Protect for StandardReclaimer {
    fn begin_critical_section(&self) {
        self.get_or_claim_slot()
            .is_in_critical_section
            .store(true, SeqCst);
    }

    fn end_critical_section(&self) {
        let slot = self.get_or_claim_slot();
        slot.is_in_critical_section.store(false, SeqCst);
        slot.primary_list.detach_head();
    }
}

impl ProtectPtr for StandardReclaimer {
    type ProtectionHandle = SnapshotPtr;
    fn protect_ptr(&self, ptr: *mut u8) -> &SnapshotPtr {
        // TODO: don't search from the beginning every time
        self.get_or_claim_slot()
            .snapshots
            .try_for_each_with_append(|s| {
                s.ptr
                    .compare_exchange(null_mut(), ptr, SeqCst, SeqCst)
                    .is_ok()
            })
    }
}

impl ProvideGlobal for StandardReclaimer {
    fn get_global() -> &'static Self {
        static GLOBAL: OnceLock<StandardReclaimer> = OnceLock::new();
        GLOBAL.get_or_init(StandardReclaimer::new)
    }
}

impl Retire for StandardReclaimer {
    fn retire(&self, ptr: *mut u8, f: Box<dyn Fn(*mut u8)>) {
        let mut borrowed = self.get_or_claim_slot().batch.borrow_mut();
        borrowed.functions.push((ptr, f));
        borrowed.lookup.insert(ptr);
        if borrowed.functions.len() < borrowed.functions.capacity() {
            return;
        }
        let next_batch_size = self.slots.get_nodes_count() * SLOTS_PER_NODE;
        let batch = mem::replace(
            borrowed.deref_mut(),
            Batch {
                functions: Vec::with_capacity(next_batch_size),
                lookup: HashSet::with_capacity(next_batch_size),
            },
        );
        // Drop the borrow before proceeding in case there is a recursive call to this function.
        drop(borrowed);
        let batch_arc = UnsafeArc::new(batch, 1);
        for slot in self.slots.iter(SeqCst) {
            if slot.is_in_critical_section.load(SeqCst) {
                // If a thread is in a critical section, it must be made aware of any retirements.
                // The snapshots will be checked when that thread exits the critical section.
                slot.primary_list.insert(batch_arc.clone(), slot);
            } else {
                // Otherwise, the snapshots can be checked immediately.
                for snapshot_ptr in slot.snapshots.iter(SeqCst) {
                    let p = snapshot_ptr.ptr.load(SeqCst);
                    if !p.is_null() && batch_arc.lookup.contains(&p) {
                        snapshot_ptr.conflicts.insert(batch_arc.clone(), null());
                    }
                }
            }
        }
    }

    fn drop_flag(&self) -> &Cell<bool> {
        &self.is_dropped
    }
}

unsafe impl Send for StandardReclaimer {}

unsafe impl Sync for StandardReclaimer {}

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
    fn insert(&self, batch: UnsafeArc<Batch>, check_on_drop: *const Slot) {
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
    fn detach_head(&self) -> bool {
        unsafe {
            let ptr = self.head.swap(null_mut(), SeqCst);
            if !ptr.is_null() {
                drop(UnsafeArc::from_raw(ptr));
            }
            !ptr.is_null()
        }
    }
}

struct CollectionNode {
    batch: UnsafeArc<Batch>,
    next: Option<UnsafeArc<CollectionNode>>,
    check_on_drop: *const Slot, // TODO: determine if this can be done without using a raw pointer
}

impl Drop for CollectionNode {
    fn drop(&mut self) {
        if !self.check_on_drop.is_null() {
            unsafe {
                for snapshot_ptr in (*self.check_on_drop).snapshots.iter(SeqCst) {
                    let ptr = snapshot_ptr.ptr.load(SeqCst);
                    if !ptr.is_null() && self.batch.lookup.contains(&ptr) {
                        // TODO: figure out how to do this by moving instead of cloning (RefCell?)
                        snapshot_ptr.conflicts.insert(self.batch.clone(), null());
                    }
                }
            }
        }
    }
}

#[allow(clippy::type_complexity)]
#[derive(Default)]
struct Batch {
    functions: Vec<(*mut u8, Box<dyn Fn(*mut u8)>)>,
    lookup: HashSet<*mut u8>,
}

impl Drop for Batch {
    fn drop(&mut self) {
        for (ptr, f) in self.functions.iter() {
            (**f)(*ptr);
        }
    }
}

#[derive(Default)]
struct SlotLookup(HashMap<*const StandardReclaimer, (*const Slot, UnsafeArc<Cell<bool>>)>);

impl Drop for SlotLookup {
    fn drop(&mut self) {
        unsafe {
            for (ptr, is_terminated) in self.0.values() {
                if !(*is_terminated).get() {
                    (**ptr).is_claimed.store(false, SeqCst);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::smr::drc::{Protect, ProtectPtr, Release, Retire};
    use crate::smr::standard_reclaimer::StandardReclaimer;
    use std::alloc::{dealloc, Layout};
    use std::cell::Cell;
    use std::ptr::{addr_of, null_mut};
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
            let r = StandardReclaimer::new();
            let dummy_ptr = addr_of!(r) as *mut u8;

            r.begin_critical_section();
            let slot = r.get_or_claim_slot();
            assert!(slot.is_in_critical_section.load(SeqCst));

            r.retire(dummy_ptr, Box::new(|_| flag.set(true)));
            assert!(!flag.get());

            r.end_critical_section();
            assert!(!slot.is_in_critical_section.load(SeqCst));

            drop(r);
            assert!(flag.get());
        });
    }

    #[test]
    fn test_protect_ptr_and_release() {
        with_flag(|flag| {
            let r = StandardReclaimer::new();
            let dummy_ptr = addr_of!(r) as *mut u8;

            let handle = r.protect_ptr(dummy_ptr);
            assert_eq!(handle.ptr.load(SeqCst), dummy_ptr);

            r.retire(dummy_ptr, Box::new(|_| flag.set(true)));
            assert!(!flag.get());

            handle.release();
            assert_eq!(handle.ptr.load(SeqCst), null_mut());

            drop(r);
            assert!(flag.get());
        });
    }
}

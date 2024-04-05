use crate::atomics::{AsPtr, CloneFromRaw};
use crate::smr::drc::ProtectPtr;
use crate::smr::standard_reclaimer::StandardReclaimer;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::ptr::NonNull;
use std::sync::{Arc, Weak};

/// An [`Arc`]-like smart pointer that facilitates reads and writes to
/// [`AtomicArc`][`crate::AtomicArc`].
///
/// Usage notes:
/// * `Snapshot` implements [`Deref`] and prevents deallocation, but it does not contribute to the
/// strong count.
///     * Consider the common use case of traversing a data structure like a tree or linked list.
/// A naive implementation using [`Arc`] would require every read operation to be sandwiched by an
/// increment and a decrement to the strong count. To eliminate this contention on the counter, we
/// can load a `Snapshot` instead, which provides protection through a hazard-pointer-like
/// mechanism, allowing us to quickly read the node and discard the `Snapshot` without touching the
/// strong count.
/// * A `Snapshot` should be used as a temporary variable within a local scope, not as a
/// replacement for [`Arc`] in a data structure.
/// * If a thread holds too many `Snapshot`s at a time, the performance of [`StandardReclaimer`]
/// may gradually degrade.
/// * A `Snapshot` can only be created through methods on [`AtomicArc`][`crate::AtomicArc`].
pub struct Snapshot<T, R: ProtectPtr = StandardReclaimer> {
    ptr: NonNull<T>,
    phantom: PhantomData<T>,
    _guard: R::Guard,
}

impl<T, R: ProtectPtr> Deref for Snapshot<T, R> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*(self.ptr.as_ptr()) }
    }
}

impl<T, R: ProtectPtr> AsPtr<T> for Snapshot<T, R> {
    fn as_ptr(this: &Self) -> *const T {
        this.ptr.as_ptr()
    }
}

impl<T, R: ProtectPtr> CloneFromRaw<T> for Snapshot<T, R> {
    unsafe fn clone_from_raw(ptr: *const T) -> Self {
        Self {
            ptr: NonNull::new_unchecked(ptr.cast_mut()),
            phantom: PhantomData,
            _guard: R::protect_ptr(ptr as *mut u8),
        }
    }
}

impl<T, R: ProtectPtr> From<&Snapshot<T, R>> for Arc<T> {
    fn from(value: &Snapshot<T, R>) -> Self {
        unsafe { Arc::clone_from_raw(Snapshot::as_ptr(value)) }
    }
}

impl<T, R: ProtectPtr> From<&Snapshot<T, R>> for Weak<T> {
    fn from(value: &Snapshot<T, R>) -> Self {
        unsafe { Weak::clone_from_raw(Snapshot::as_ptr(value)) }
    }
}

use crate::smr::drc::{ProtectPtr, Release, Retire};
use crate::smr::standard_reclaimer::StandardReclaimer;
use crate::utils::helpers::alloc_box_ptr;
use std::alloc::{dealloc, Layout};
use std::marker::PhantomData;
use std::ops::Deref;
use std::ptr::NonNull;
use std::sync::atomic::Ordering::{Acquire, Relaxed, SeqCst};
use std::sync::atomic::{fence, AtomicUsize};
use std::{mem, ptr};

/// A reimplementation of [`std::sync::Arc`].
///
/// This module's `Arc` is essentially identical to the standard library's, with just
/// two constraints:
///
/// - `T` has a `'static` lifetime bound, as the `Arc` might not be destroyed immediately when the
/// reference count reaches zero.
/// - `T` must be [`Sized`] for compatability with [`AtomicArc`], which wraps [`AtomicPtr`],
/// which also has this bound.
///
/// See [`std::sync::Arc`] for per-method documentation.
///
/// # Examples:
/// ```
/// use aarc::Arc;
///
/// let x = Arc::new(53);
/// assert_eq!(*x, 53);
///
/// let y = Arc::new(53);
/// assert_eq!(*x, *y);
///
/// assert!(!Arc::ptr_eq(&x, &y));
///
/// let w = Arc::downgrade(&x);
/// assert_eq!(Arc::weak_count(&x), 1);
/// ```
///
/// [`AtomicArc`]: `super::AtomicArc`
/// [`AtomicPtr`]: `std::sync::atomic::AtomicPtr`
pub struct Arc<T: 'static, R: Retire = StandardReclaimer> {
    ptr: NonNull<ArcInner<T>>,
    phantom: PhantomData<ArcInner<T>>,
    phantom_r: PhantomData<R>,
}

impl<T: 'static> Arc<T, StandardReclaimer> {
    pub fn new(data: T) -> Self {
        Arc::<_, StandardReclaimer>::new_in(data)
    }
}

impl<T: 'static, R: Retire> Arc<T, R> {
    pub fn downgrade(this: &Arc<T, R>) -> Weak<T, R> {
        unsafe { Weak::clone_from_raw(this.ptr.as_ptr().cast()) }
    }
    #[allow(clippy::missing_safety_doc)]
    pub unsafe fn from_raw(ptr: *const T) -> Self {
        Self {
            ptr: NonNull::new_unchecked(ptr as *mut ArcInner<T>),
            phantom: PhantomData,
            phantom_r: PhantomData,
        }
    }
    #[allow(clippy::missing_safety_doc)]
    pub unsafe fn increment_strong_count(ptr: *const T) {
        (*(ptr as *const ArcInner<T>)).increment_strong_count();
    }
    pub fn into_raw(this: Self) -> *const T {
        let ptr = Self::as_ptr(&this);
        mem::forget(this);
        ptr
    }
    pub fn new_in(data: T) -> Self {
        unsafe {
            Self {
                ptr: NonNull::new_unchecked(alloc_box_ptr(ArcInner {
                    data,
                    strong: AtomicUsize::new(1),
                    weak: AtomicUsize::new(1),
                })),
                phantom: PhantomData,
                phantom_r: PhantomData,
            }
        }
    }
    pub fn ptr_eq(this: &Self, other: &Self) -> bool {
        ptr::eq(Self::as_ptr(this), Self::as_ptr(other))
    }
    pub fn strong_count(this: &Self) -> usize {
        unsafe { (*this.ptr.as_ptr()).strong.load(Relaxed) }
    }
    pub fn weak_count(this: &Self) -> usize {
        unsafe { (*this.ptr.as_ptr()).weak.load(Relaxed) - 1 }
    }
    pub(crate) unsafe fn try_increment_strong_count(ptr: *const T) -> bool {
        (*(ptr as *const ArcInner<T>))
            .strong
            .fetch_update(Acquire, Relaxed, |n| (n != 0).then_some(n + 1))
            .is_ok()
    }
}

impl<T: 'static, R: Retire> Clone for Arc<T, R> {
    fn clone(&self) -> Self {
        unsafe { Self::clone_from_raw(self.ptr.as_ptr().cast()) }
    }
}

impl<T: 'static, R: Retire> Deref for Arc<T, R> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*(self.ptr.as_ptr() as *mut T) }
    }
}

impl<T: 'static, R: Retire> Drop for Arc<T, R> {
    fn drop(&mut self) {
        unsafe {
            let inner = self.ptr.as_ptr();
            if (*inner).strong.fetch_sub(1, SeqCst) == 1 {
                fence(Acquire);
                R::retire(
                    inner as *mut u8,
                    Box::new(move || {
                        if (*inner).strong.load(SeqCst) == 0 {
                            ptr::drop_in_place(inner as *mut T);
                            drop(Weak::<T, R>::from_raw(inner as *const T));
                        }
                    }),
                );
            }
        }
    }
}

unsafe impl<T: 'static + Send + Sync, R: Retire> Send for Arc<T, R> {}

unsafe impl<T: 'static + Send + Sync, R: Retire> Sync for Arc<T, R> {}

/// A reimplementation of [`std::sync::Weak`].
///
/// See [`Arc`] for details on how this struct differs from the standard library's.
///
/// See [`std::sync::Weak`] for per-method documentation.
pub struct Weak<T: 'static, R: Retire = StandardReclaimer> {
    ptr: NonNull<ArcInner<T>>,
    phantom_r: PhantomData<R>,
}

impl<T: 'static, R: Retire> Weak<T, R> {
    #[allow(clippy::missing_safety_doc)]
    pub unsafe fn from_raw(ptr: *const T) -> Self {
        Self {
            ptr: NonNull::new_unchecked(ptr as *mut ArcInner<T>),
            phantom_r: PhantomData,
        }
    }
    pub(crate) unsafe fn increment_weak_count(ptr: *const T) {
        (*(ptr as *const ArcInner<T>)).increment_weak_count();
    }
    pub fn into_raw(self) -> *const T {
        let ptr = Self::as_ptr(&self);
        mem::forget(self);
        ptr
    }
    pub fn upgrade(&self) -> Option<Arc<T, R>> {
        unsafe {
            (*self.ptr.as_ptr())
                .strong
                .fetch_update(Acquire, Relaxed, |n| (n != 0).then_some(n + 1))
                .ok()?;
            Some(Arc {
                ptr: self.ptr,
                phantom: PhantomData,
                phantom_r: PhantomData,
            })
        }
    }
}

impl<T: 'static, R: Retire> Drop for Weak<T, R> {
    fn drop(&mut self) {
        unsafe {
            let inner = self.ptr.as_ptr();
            if (*inner).weak.fetch_sub(1, SeqCst) == 1 {
                fence(Acquire);
                R::retire(
                    inner as *mut u8,
                    Box::new(move || {
                        if (*inner).weak.load(SeqCst) == 0 {
                            dealloc(inner as *mut u8, Layout::new::<ArcInner<T>>())
                        }
                    }),
                );
            }
        }
    }
}

unsafe impl<T: 'static + Send + Sync, R: Retire> Send for Weak<T, R> {}

unsafe impl<T: 'static + Send + Sync, R: Retire> Sync for Weak<T, R> {}

/// An [`Arc`]-like pointer that facilitates reads and writes to [`AtomicArc`] and [`AtomicWeak`].
///
/// Like [`Arc`], `Snapshot` provides an immutable reference `&T` and prevents deallocation, but
/// it does *not* affect reference counts.
///
/// Consider, for example, the process of traversing a tree or linked list. If an [`Arc`] (instead
/// of a `Snapshot`) were loaded from each [`AtomicArc`], every visit to a node would be sandwiched
/// by an increment and a decrement. `Snapshot`s eliminate this contention.
///
/// A `Snapshot` should be used as a temporary variable. **It should not be used in place of
/// [`Arc`] or [`AtomicArc`] in a data structure**. In addition, if a thread holds too
/// many `Snapshot`s at a time, the performance of [`StandardReclaimer`] may gradually degrade.
///
/// The only way to obtain one is to `load` an [`AtomicArc`] or `upgrade` an [`AtomicWeak`].
///
/// [`AtomicArc`]: `super::AtomicArc`
/// [`AtomicWeak`]: `super::AtomicWeak`
pub struct Snapshot<T: 'static, R: ProtectPtr = StandardReclaimer> {
    ptr: NonNull<ArcInner<T>>,
    phantom: PhantomData<ArcInner<T>>,
    handle: &'static R::ProtectionHandle,
}

impl<T: 'static, R: ProtectPtr> Clone for Snapshot<T, R> {
    fn clone(&self) -> Self {
        unsafe { Self::clone_from_raw(Self::as_ptr(self)) }
    }
}

impl<T: 'static, R: ProtectPtr> Deref for Snapshot<T, R> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*(self.ptr.as_ptr() as *mut T) }
    }
}

impl<T: 'static, R: ProtectPtr> Drop for Snapshot<T, R> {
    fn drop(&mut self) {
        self.handle.release();
    }
}

#[repr(C)]
pub(crate) struct ArcInner<T> {
    data: T,
    strong: AtomicUsize,
    weak: AtomicUsize,
}

impl<T> ArcInner<T> {
    pub(crate) fn increment_strong_count(&self) {
        self.strong.fetch_add(1, Relaxed);
    }
    pub(crate) fn increment_weak_count(&self) {
        self.weak.fetch_add(1, Relaxed);
    }
}

/// A trait to wrap the `as_ptr` method. See [`std::sync::Arc::as_ptr`].
pub trait AsPtr<T> {
    /// Extracts an object's raw pointer.
    fn as_ptr(this: &Self) -> *const T;
}

impl<T: 'static, R: Retire> AsPtr<T> for Arc<T, R> {
    fn as_ptr(this: &Self) -> *const T {
        this.ptr.as_ptr() as *const T
    }
}

impl<T: 'static, R: Retire> AsPtr<T> for Weak<T, R> {
    fn as_ptr(this: &Self) -> *const T {
        this.ptr.as_ptr() as *const T
    }
}

impl<T: 'static, R: ProtectPtr> AsPtr<T> for Snapshot<T, R> {
    fn as_ptr(this: &Self) -> *const T {
        this.ptr.as_ptr() as *const T
    }
}

pub trait CloneFromRaw<T> {
    unsafe fn clone_from_raw(ptr: *const T) -> Self;
}

impl<T: 'static, R: Retire> CloneFromRaw<T> for Arc<T, R> {
    unsafe fn clone_from_raw(ptr: *const T) -> Self {
        Self::increment_strong_count(ptr);
        Self::from_raw(ptr)
    }
}

impl<T: 'static, R: Retire> CloneFromRaw<T> for Weak<T, R> {
    unsafe fn clone_from_raw(ptr: *const T) -> Self {
        Self::increment_weak_count(ptr);
        Self::from_raw(ptr)
    }
}

impl<T: 'static, R: ProtectPtr> CloneFromRaw<T> for Snapshot<T, R> {
    unsafe fn clone_from_raw(ptr: *const T) -> Self {
        Self {
            ptr: NonNull::new_unchecked(ptr as *mut ArcInner<T>),
            phantom: PhantomData,
            handle: R::protect_ptr(ptr as *mut u8),
        }
    }
}

pub trait TryCloneFromRaw<T>: Sized {
    unsafe fn try_clone_from_raw(ptr: *const T) -> Option<Self>;
}

impl<T: 'static, R: Retire> TryCloneFromRaw<T> for Arc<T, R> {
    unsafe fn try_clone_from_raw(ptr: *const T) -> Option<Self> {
        Self::try_increment_strong_count(ptr).then_some(Self::from_raw(ptr))
    }
}

impl<T: 'static, R: ProtectPtr> TryCloneFromRaw<T> for Snapshot<T, R> {
    unsafe fn try_clone_from_raw(ptr: *const T) -> Option<Self> {
        let inner = ptr as *mut ArcInner<T>;
        let handle = R::protect_ptr(ptr as *mut u8);
        if (*inner).strong.load(SeqCst) == 0 {
            handle.release();
            return None;
        }
        Some(Self {
            ptr: NonNull::new_unchecked(inner),
            phantom: PhantomData,
            handle,
        })
    }
}

impl<T: 'static, R: ProtectPtr + Retire> From<&Arc<T, R>> for Snapshot<T, R> {
    fn from(value: &Arc<T, R>) -> Self {
        unsafe { Self::clone_from_raw(Arc::as_ptr(value)) }
    }
}

impl<T: 'static, R: ProtectPtr + Retire> From<&Snapshot<T, R>> for Arc<T, R> {
    fn from(value: &Snapshot<T, R>) -> Self {
        unsafe { Self::clone_from_raw(Snapshot::as_ptr(value)) }
    }
}

#[cfg(test)]
mod tests {
    use crate::smr::standard_reclaimer::StandardReclaimer;
    use crate::{Arc, Weak};
    use std::cell::RefCell;

    #[test]
    fn test_arc_cascading_drop() {
        struct Node {
            _next: Option<Arc<Self>>,
        }
        let _node0 = Arc::new(Node {
            _next: Some(Arc::new(Node { _next: None })),
        });
        unsafe {
            StandardReclaimer::cleanup();
        }
    }

    #[test]
    fn test_arc_weak_cycle() {
        struct Node {
            _prev: Option<Weak<RefCell<Self>>>,
            _next: Option<Arc<RefCell<Self>>>,
        }
        let n0 = Arc::new(RefCell::new(Node {
            _prev: None,
            _next: None,
        }));
        let n1 = Arc::new(RefCell::new(Node {
            _prev: Some(Arc::downgrade(&n0)),
            _next: None,
        }));
        n0.borrow_mut()._next = Some(n1.clone());
        unsafe {
            StandardReclaimer::cleanup();
        }
    }
}

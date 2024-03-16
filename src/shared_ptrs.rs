use crate::smr::drc::{ProtectPtr, ProvideGlobal, Release, Retire};
use crate::smr::standard_reclaimer::StandardReclaimer;
use crate::utils::helpers::alloc_box_ptr;
use std::alloc::{dealloc, Layout};
use std::cell::Cell;
use std::marker::PhantomData;
use std::ops::Deref;
use std::ptr::NonNull;
use std::sync::atomic::Ordering::{Acquire, Relaxed, SeqCst};
use std::sync::atomic::{fence, AtomicUsize};
use std::{mem, ptr};

/// A reimplementation of [`std::sync::Arc`].
///
/// This module's `Arc` is essentially identical to [`std::sync::Arc`], with just two constraints:
///
/// - `T` must be [`Sized`]. [`AtomicArc`] wraps [`AtomicPtr`], which also has this restriction.
/// - `T` has a lifetime bound `'a`, tied to the `Arc`'s reclaimer `R` which handles memory
/// management.
///
/// [`AtomicArc`]: `super::AtomicArc`
/// [`AtomicPtr`]: `std::sync::atomic::AtomicPtr`
pub struct Arc<'a, T: 'a, R: Retire = StandardReclaimer> {
    ptr: NonNull<ArcInner<'a, T, R>>,
    phantom: PhantomData<ArcInner<'a, T, R>>,
}

impl<T: 'static> Arc<'static, T, StandardReclaimer> {
    /// Creates a new `Arc` linked to the default global reclaimer. Alternatively, `new_in` can be
    /// used if a `'static` bound is too restrictive, or if separate reclaimer instances would
    /// improve performance (e.g. two thread pools working on two disconnected data structures).
    ///
    /// # Examples:
    ///
    /// ```
    /// use aarc::Arc;
    ///
    /// let x = Arc::new(53);
    /// assert_eq!(*x, 53);
    /// ```
    pub fn new(data: T) -> Self {
        Arc::new_in(data, StandardReclaimer::get_global())
    }
}

impl<'a, T: 'a, R: Retire> Arc<'a, T, R> {
    pub fn downgrade(this: &Arc<'a, T, R>) -> Weak<'a, T, R> {
        unsafe { Weak::clone_from_raw(this.ptr.as_ptr().cast()) }
    }
    /// Refer to the standard library:
    /// [`Arc::from_raw`][`std::sync::Arc::from_raw`].
    #[allow(clippy::missing_safety_doc)]
    pub unsafe fn from_raw(ptr: *const T) -> Self {
        Self {
            ptr: NonNull::new_unchecked(ptr as *mut ArcInner<T, R>),
            phantom: PhantomData,
        }
    }
    /// Refer to the standard library:
    /// [`Arc::increment_strong_count`][`std::sync::Arc::increment_strong_count`].
    #[allow(clippy::missing_safety_doc)]
    pub unsafe fn increment_strong_count(ptr: *const T) {
        (*(ptr as *const ArcInner<T, R>))
            .strong
            .fetch_add(1, SeqCst);
    }
    /// Refer to the standard library:
    /// [`Arc::into_raw`][`std::sync::Arc::into_raw`].
    pub fn into_raw(this: Self) -> *const T {
        let ptr = Self::as_ptr(&this);
        mem::forget(this);
        ptr
    }
    /// Creates a new `Arc` linked to the provided reclaimer.
    ///
    /// The [`standard_reclaimer_newtype`][`super::standard_reclaimer_newtype`] macro should be
    /// used when instantiating new [`StandardReclaimer`] instances.
    ///
    /// # Examples:
    /// ```
    /// use aarc::Arc;
    /// use aarc::standard_reclaimer_newtype;
    ///
    /// standard_reclaimer_newtype!(MyReclaimer);
    /// let r = MyReclaimer::new();
    ///
    /// let x = Arc::new_in(53, &r);
    /// assert_eq!(*x, 53);
    /// ```
    pub fn new_in(data: T, reclaimer: &'a R) -> Self {
        unsafe {
            Self {
                ptr: NonNull::new_unchecked(alloc_box_ptr(ArcInner {
                    data,
                    strong: AtomicUsize::new(1),
                    weak: AtomicUsize::new(1),
                    reclaimer,
                    drop_flag: reclaimer.drop_flag(),
                })),
                phantom: PhantomData,
            }
        }
    }
    /// Refer to the standard library:
    /// [`Arc::strong_count`][`std::sync::Arc::strong_count`].
    pub fn strong_count(this: &Self) -> usize {
        unsafe { (*this.ptr.as_ptr()).strong.load(Relaxed) }
    }
    /// Refer to the standard library:
    /// [`Arc::weak_count`][`std::sync::Arc::weak_count`].
    pub fn weak_count(this: &Self) -> usize {
        unsafe { (*this.ptr.as_ptr()).weak.load(Relaxed) - 1 }
    }
    pub(crate) unsafe fn try_increment_strong_count(ptr: *const T) -> bool {
        (*(ptr as *const ArcInner<T, R>))
            .strong
            .fetch_update(Acquire, Relaxed, |n| (n != 0).then_some(n + 1))
            .is_ok()
    }
}

impl<'a, T: 'a, R: Retire> Clone for Arc<'a, T, R> {
    fn clone(&self) -> Self {
        unsafe { Self::clone_from_raw(self.ptr.as_ptr().cast()) }
    }
}

impl<'a, T: 'a, R: Retire> Deref for Arc<'a, T, R> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*(self.ptr.as_ptr() as *mut T) }
    }
}

impl<'a, T: 'a, R: Retire> Drop for Arc<'a, T, R> {
    fn drop(&mut self) {
        unsafe {
            let inner = self.ptr.as_ptr();
            if (*inner).strong.fetch_sub(1, SeqCst) == 1 {
                fence(Acquire);
                if (*inner).drop_flag.get() {
                    ptr::drop_in_place::<T>(inner.cast());
                    drop(Weak::<T, R>::from_raw(inner.cast()));
                } else {
                    (*inner).reclaimer.retire(
                        inner.cast(),
                        Box::new(|p| {
                            ptr::drop_in_place::<T>(p.cast());
                            drop(Weak::<T, R>::from_raw(p.cast()));
                        }),
                    );
                }
            }
        }
    }
}

unsafe impl<'a, T: 'a + Send + Sync, R: Retire> Send for Arc<'a, T, R> {}

unsafe impl<'a, T: 'a + Send + Sync, R: Retire> Sync for Arc<'a, T, R> {}

/// A reimplementation of [`std::sync::Weak`].
///
/// See [`Arc`] for details on how this struct differs from [`std::sync::Weak`].
pub struct Weak<'a, T: 'a, R: Retire = StandardReclaimer> {
    ptr: NonNull<ArcInner<'a, T, R>>,
}

impl<'a, T: 'a, R: Retire> Weak<'a, T, R> {
    /// Refer to the standard library:
    /// [`Weak::from_raw`][`std::sync::Weak::from_raw`].
    #[allow(clippy::missing_safety_doc)]
    pub unsafe fn from_raw(ptr: *const T) -> Self {
        Self {
            ptr: NonNull::new_unchecked(ptr as *mut ArcInner<T, R>),
        }
    }
    pub(crate) unsafe fn increment_weak_count(ptr: *const T) {
        (*(ptr as *const ArcInner<T, R>)).weak.fetch_add(1, SeqCst);
    }
    /// Refer to the standard library:
    /// [`Weak::into_raw`][`std::sync::Weak::into_raw`].
    pub fn into_raw(self) -> *const T {
        let ptr = Self::as_ptr(&self);
        mem::forget(self);
        ptr
    }
    /// Refer to the standard library:
    /// [`Weak::upgrade`][`std::sync::Weak::upgrade`].
    pub fn upgrade(&self) -> Option<Arc<'a, T, R>> {
        unsafe {
            (*self.ptr.as_ptr())
                .strong
                .fetch_update(Acquire, Relaxed, |n| (n != 0).then_some(n + 1))
                .ok()?;
            Some(Arc {
                ptr: self.ptr,
                phantom: PhantomData,
            })
        }
    }

    /*
    unsafe fn decrement_weak_count(ptr: *const T, drop_flag: bool, reclaimer: *const R) {
        if (*(ptr as *const ArcInner<T, R>)).weak.fetch_sub(1, SeqCst) == 1 {
            fence(Acquire);
            if drop_flag {
                dealloc(ptr as *mut u8, Layout::new::<ArcInner<T, R>>());
            } else {
                reclaimer.retire(
                    ptr as *mut u8,
                    Box::new(|p| dealloc(p, Layout::new::<ArcInner<T, R>>())),
                );
            }
        }
    }
    */
}

impl<'a, T: 'a, R: Retire> Drop for Weak<'a, T, R> {
    fn drop(&mut self) {
        unsafe {
            let inner = self.ptr.as_ptr();
            if (*inner).weak.fetch_sub(1, SeqCst) == 1 {
                fence(Acquire);
                if (*inner).drop_flag.get() {
                    dealloc(inner.cast(), Layout::new::<ArcInner<T, R>>());
                } else {
                    (*inner).reclaimer.retire(
                        inner.cast(),
                        Box::new(|p| dealloc(p, Layout::new::<ArcInner<T, R>>())),
                    );
                }
            }
        }
    }
}

unsafe impl<'a, T: 'a + Send + Sync, R: Retire> Send for Weak<'a, T, R> {}

unsafe impl<'a, T: 'a + Send + Sync, R: Retire> Sync for Weak<'a, T, R> {}

/// An [`Arc`]-like pointer that facilitates reads and writes to [`AtomicArc`].
///
/// Like [`Arc`] and [`std::sync::Arc`], `Snapshot` provides an immutable reference to the wrapped
/// object, but it does not contribute to the reference count.
///
/// If one were to, for example, traverse a tree or linked list by loading an [`Arc`] from each
/// [`AtomicArc`] (instead of loading a `Snapshot`), every visit to a node would need to be
/// sandwiched by an increment and a decrement to the reference count. `Snapshot`s accelerate data
/// structure operations by eliminating this contention.
///
/// A `Snapshot` should be used as a temporary variable. **It should not be used in place of
/// [`Arc`] or [`AtomicArc`] in the data structure itself**. In addition, if a thread holds too
/// many `Snapshot`s at a time, the performance of [`AtomicArc`] may gradually degrade.
///
/// [`AtomicArc`]: `super::AtomicArc`
pub struct Snapshot<'a, T: 'a, R: ProtectPtr = StandardReclaimer> {
    ptr: NonNull<ArcInner<'a, T, R>>,
    phantom: PhantomData<ArcInner<'a, T, R>>,
    handle: &'a R::ProtectionHandle,
}

impl<'a, T: 'a, R: ProtectPtr> Clone for Snapshot<'a, T, R> {
    fn clone(&self) -> Self {
        unsafe { Self::clone_from_raw(Self::as_ptr(self)) }
    }
}

impl<'a, T: 'a, R: ProtectPtr> Deref for Snapshot<'a, T, R> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*(self.ptr.as_ptr() as *mut T) }
    }
}

impl<'a, T: 'a, R: ProtectPtr> Drop for Snapshot<'a, T, R> {
    fn drop(&mut self) {
        self.handle.release();
    }
}

#[repr(C)]
pub(crate) struct ArcInner<'a, T: 'a, R> {
    data: T,
    strong: AtomicUsize,
    weak: AtomicUsize,
    reclaimer: &'a R,
    drop_flag: &'a Cell<bool>,
}

impl<'a, T: 'a, R> ArcInner<'a, T, R> {
    pub(crate) fn increment_strong_count(&self) {
        self.strong.fetch_add(1, Relaxed);
    }
    pub(crate) fn increment_weak_count(&self) {
        self.weak.fetch_add(1, Relaxed);
    }
    pub(crate) fn reclaimer(&self) -> &'a R {
        self.reclaimer
    }
}

/// A trait to wrap the `as_ptr` method. See [`std::sync::Arc::as_ptr`].
pub trait AsPtr<T> {
    /// Extracts an object's raw pointer.
    fn as_ptr(this: &Self) -> *const T;
}

impl<'a, T: 'a, R: Retire> AsPtr<T> for Arc<'a, T, R> {
    fn as_ptr(this: &Self) -> *const T {
        this.ptr.as_ptr() as *const T
    }
}

impl<'a, T: 'a, R: Retire> AsPtr<T> for Weak<'a, T, R> {
    fn as_ptr(this: &Self) -> *const T {
        this.ptr.as_ptr() as *const T
    }
}

impl<'a, T: 'a, R: ProtectPtr> AsPtr<T> for Snapshot<'a, T, R> {
    fn as_ptr(this: &Self) -> *const T {
        this.ptr.as_ptr() as *const T
    }
}

pub trait CloneFromRaw<T> {
    unsafe fn clone_from_raw(ptr: *const T) -> Self;
}

impl<'a, T: 'a, R: Retire> CloneFromRaw<T> for Arc<'a, T, R> {
    unsafe fn clone_from_raw(ptr: *const T) -> Self {
        unsafe {
            Self::increment_strong_count(ptr);
            Self::from_raw(ptr)
        }
    }
}

impl<'a, T: 'a, R: Retire> CloneFromRaw<T> for Weak<'a, T, R> {
    unsafe fn clone_from_raw(ptr: *const T) -> Self {
        unsafe {
            Self::increment_weak_count(ptr);
            Self::from_raw(ptr)
        }
    }
}

impl<'a, T: 'a, R: ProtectPtr> CloneFromRaw<T> for Snapshot<'a, T, R> {
    unsafe fn clone_from_raw(ptr: *const T) -> Self {
        Self {
            ptr: NonNull::new_unchecked(ptr as *mut ArcInner<T, R>),
            phantom: PhantomData,
            handle: (*(ptr as *mut ArcInner<T, R>))
                .reclaimer
                .protect_ptr(ptr as *mut u8),
        }
    }
}

pub trait TryCloneFromRaw<T>: Sized {
    fn try_clone_from_raw(ptr: *const T) -> Option<Self>;
}

impl<'a, T: 'a, R: Retire> TryCloneFromRaw<T> for Arc<'a, T, R> {
    fn try_clone_from_raw(ptr: *const T) -> Option<Self> {
        unsafe { Self::try_increment_strong_count(ptr).then_some(Self::from_raw(ptr)) }
    }
}

impl<'a, T: 'a, R: ProtectPtr> TryCloneFromRaw<T> for Snapshot<'a, T, R> {
    fn try_clone_from_raw(ptr: *const T) -> Option<Self> {
        unsafe {
            let inner = &*(ptr as *mut ArcInner<T, R>);
            if inner.strong.load(SeqCst) == 0 {
                return None;
            }
            let handle = inner.reclaimer.protect_ptr(ptr as *mut u8);
            if inner.strong.load(SeqCst) == 0 {
                handle.release();
                return None;
            }
            Some(Self {
                ptr: NonNull::new_unchecked(ptr as *mut ArcInner<T, R>),
                phantom: PhantomData,
                handle,
            })
        }
    }
}

impl<'a, T: 'a, R: ProtectPtr + Retire> From<&Arc<'a, T, R>> for Snapshot<'a, T, R> {
    fn from(value: &Arc<'a, T, R>) -> Self {
        unsafe { Self::clone_from_raw(Arc::as_ptr(value)) }
    }
}

impl<'a, T: 'a, R: ProtectPtr + Retire> From<&Snapshot<'a, T, R>> for Arc<'a, T, R> {
    fn from(value: &Snapshot<'a, T, R>) -> Self {
        unsafe { Self::clone_from_raw(Snapshot::as_ptr(value)) }
    }
}

#[cfg(test)]
mod tests {
    use crate::{standard_reclaimer_newtype, Arc, Weak};
    use std::cell::RefCell;

    #[test]
    fn test_arc_cascading_drop() {
        standard_reclaimer_newtype!(MyReclaimer);
        let r = MyReclaimer::new();
        struct Node<'a> {
            _next: Option<Arc<'a, Self, MyReclaimer>>,
        }
        let _node0 = Arc::new_in(
            Node {
                _next: Some(Arc::new_in(Node { _next: None }, &r)),
            },
            &r,
        );
    }

    #[test]
    fn test_dealloc_with_weak() {
        standard_reclaimer_newtype!(MyReclaimer1);
        let r = MyReclaimer1::new();

        struct Node<'a> {
            _prev: Option<Weak<'a, RefCell<Self>, MyReclaimer1>>,
            _next: Option<Arc<'a, RefCell<Self>, MyReclaimer1>>,
        }
        let n0 = Arc::new_in(
            RefCell::new(Node {
                _prev: None,
                _next: None,
            }),
            &r,
        );
        let n1 = Arc::new_in(
            RefCell::new(Node {
                _prev: Some(Arc::downgrade(&n0)),
                _next: None,
            }),
            &r,
        );
        n0.borrow_mut()._next = Some(n1.clone());
    }
}

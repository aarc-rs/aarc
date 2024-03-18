use crate::shared_ptrs::{ArcInner, AsPtr, CloneFromRaw, TryCloneFromRaw};
use crate::smr::drc::{Protect, ProtectPtr, Retire};
use crate::smr::standard_reclaimer::StandardReclaimer;
use crate::{Arc, Snapshot, Weak};
use std::marker::PhantomData;
use std::ptr;
use std::ptr::{null, null_mut};
use std::sync::atomic::Ordering::SeqCst;
use std::sync::atomic::{AtomicPtr, Ordering};

/// An atomically updatable [`Arc`].
///
/// # Examples
/// ```
/// use std::sync::atomic::Ordering::SeqCst;
/// use aarc::{Arc, AtomicArc, Snapshot};
///
/// let atomic = AtomicArc::new(Some(53)); // +1 strong count on val 53
///
/// let snapshot53 = atomic.load::<Snapshot<_>>(SeqCst); // snapshot doesn't affect counts
/// assert_eq!(*snapshot53.unwrap(), 53);
///
/// let arc53 = atomic.load::<Arc<_>>(SeqCst).unwrap(); // +1 strong count on val 53
/// assert_eq!(*arc53, 53);
/// assert_eq!(Arc::strong_count(&arc53), 2);
///
/// let arc75 = Arc::new(75); // +1 strong count on val 75
/// atomic.store(Some(&arc75), SeqCst); // +1 strong on 75, -1 strong on 53
/// assert_eq!(Arc::strong_count(&arc53), 1);
/// assert_eq!(Arc::strong_count(&arc75), 2);
///
/// let snapshot75 = atomic.load::<Snapshot<_>>(SeqCst);
/// assert_eq!(*snapshot75.unwrap(), 75);
/// ```
pub struct AtomicArc<T: 'static, R: Protect + Retire = StandardReclaimer> {
    ptr: AtomicPtr<T>,
    phantom: PhantomData<T>,
    phantom_r: PhantomData<R>,
}

impl<T: 'static> AtomicArc<T, StandardReclaimer> {
    /// Similar to [`Arc::new`], but [`None`] is a valid input, in which case the `AtomicArc` will
    /// be empty to represent a null pointer.
    ///
    /// To create an `AtomicArc` from an existing [`Arc`], use `from`.
    pub fn new(data: Option<T>) -> Self {
        let ptr = data.map_or(null(), |x| Arc::into_raw(Arc::new(x)));
        Self {
            ptr: AtomicPtr::new(ptr.cast_mut()),
            phantom: PhantomData,
            phantom_r: PhantomData,
        }
    }
}

impl<T: 'static, R: Protect + Retire> AtomicArc<T, R> {
    /// Stores `new`'s pointer into `self` if `self` and `current` point to the same allocation.
    ///
    /// If the comparison succeeds, the return value will be an [`Ok`] containing the unit type
    /// (instead of a redundant copy of `current`).
    pub fn compare_exchange<C, N, V>(
        &self,
        current: Option<&C>,
        new: Option<&N>,
        success: Ordering,
        failure: Ordering,
    ) -> Result<(), Option<V>>
    where
        C: Strong<T>,
        N: Strong<T>,
        V: Strong<T>,
    {
        let c: *const T = current.map_or(null(), C::as_ptr);
        let n: *const T = new.map_or(null(), N::as_ptr);
        match with_critical_section::<R, _, _>(|| {
            self.ptr
                .compare_exchange(c.cast_mut(), n.cast_mut(), success, failure)
                .map(|before| unsafe {
                    if ptr::eq(n, before) {
                        null_mut()
                    } else {
                        if !n.is_null() {
                            Arc::<_, R>::increment_strong_count(n);
                        }
                        before
                    }
                })
        }) {
            Ok(before) => unsafe {
                if !before.is_null() {
                    drop(Arc::<_, R>::from_raw(before));
                }
                Ok(())
            },
            Err(before) => {
                if before.is_null() {
                    Err(None)
                } else {
                    unsafe { Err(Some(V::clone_from_raw(before))) }
                }
            }
        }
    }

    /// Loads the pointer and returns the desired type (`Arc` or `Snapshot`), or [`None`] if it is
    /// null.
    pub fn load<V: Strong<T>>(&self, order: Ordering) -> Option<V> {
        with_critical_section::<R, _, _>(|| {
            let ptr = self.ptr.load(order);
            if ptr.is_null() {
                None
            } else {
                unsafe { Some(V::clone_from_raw(ptr)) }
            }
        })
    }

    /// Stores `new`'s pointer (or [`None`]) into `self`.
    pub fn store<N: Strong<T>>(&self, new: Option<&N>, order: Ordering) {
        let ptr: *const T = new.map_or(null(), N::as_ptr);
        if !ptr.is_null() {
            unsafe {
                Arc::<_, R>::increment_strong_count(ptr);
            }
        }
        let before = self.ptr.swap(ptr.cast_mut(), order);
        if !before.is_null() {
            unsafe {
                drop(Arc::<_, R>::from_raw(before));
            }
        }
    }
}

impl<T: 'static, R: Protect + Retire> Clone for AtomicArc<T, R> {
    fn clone(&self) -> Self {
        let ptr = with_critical_section::<R, _, _>(|| {
            let ptr = self.ptr.load(SeqCst);
            if !ptr.is_null() {
                unsafe {
                    Arc::<_, R>::increment_strong_count(ptr);
                }
            }
            ptr
        });
        Self {
            ptr: AtomicPtr::new(ptr),
            phantom: PhantomData,
            phantom_r: PhantomData,
        }
    }
}

impl<T: 'static> Default for AtomicArc<T, StandardReclaimer> {
    fn default() -> Self {
        Self {
            ptr: AtomicPtr::default(),
            phantom: PhantomData,
            phantom_r: PhantomData,
        }
    }
}

impl<T: 'static, R: Protect + Retire> Drop for AtomicArc<T, R> {
    fn drop(&mut self) {
        let ptr = self.ptr.load(SeqCst);
        if !ptr.is_null() {
            unsafe {
                drop(Arc::<_, R>::from_raw(ptr));
            }
        }
    }
}

/// An atomically updatable [`Weak`].
///
/// # Examples
/// ```
/// use std::sync::atomic::Ordering::SeqCst;
/// use aarc::{Arc, AtomicArc, AtomicWeak, Snapshot};
///
/// let arc1 = Arc::new(53); // +1 strong count
///
/// let atomic = AtomicWeak::default();
/// atomic.store(Some(&arc1), SeqCst); // +1 weak count
///
/// let snapshot = atomic.upgrade::<Snapshot<_>>(SeqCst); // snapshot doesn't affect counts
/// assert_eq!(*snapshot.unwrap(), 53);
///
/// let weak = atomic.load(SeqCst).unwrap(); // +1 weak count
/// let arc2 = weak.upgrade().unwrap(); // +1 strong count
/// assert_eq!(*arc2, 53);
/// assert_eq!(Arc::strong_count(&arc2), 2);
/// assert_eq!(Arc::weak_count(&arc2), 2);
/// ```
pub struct AtomicWeak<T: 'static, R: Protect + Retire = StandardReclaimer> {
    ptr: AtomicPtr<T>,
    phantom_r: PhantomData<R>,
}

impl<T: 'static, R: Protect + Retire> AtomicWeak<T, R> {
    /// See [`AtomicArc::compare_exchange`]. This method behaves similarly, except that the return
    /// type for the failure case cannot be specified by the caller; it must be a [`Weak`].
    pub fn compare_exchange<C, N>(
        &self,
        current: Option<&C>,
        new: Option<&N>,
        success: Ordering,
        failure: Ordering,
    ) -> Result<(), Option<Weak<T, R>>>
    where
        C: Shared<T>,
        N: Shared<T>,
    {
        let c: *const T = current.map_or(null(), C::as_ptr);
        let n: *const T = new.map_or(null(), N::as_ptr);
        match with_critical_section::<R, _, _>(|| {
            self.ptr
                .compare_exchange(c.cast_mut(), n.cast_mut(), success, failure)
                .map(|before| unsafe {
                    if ptr::eq(n, before) {
                        null_mut()
                    } else {
                        if !n.is_null() {
                            Weak::<_, R>::increment_weak_count(n);
                        }
                        before
                    }
                })
        }) {
            Ok(before) => unsafe {
                if !before.is_null() {
                    drop(Weak::<_, R>::from_raw(before));
                }
                Ok(())
            },
            Err(before) => {
                if before.is_null() {
                    Err(None)
                } else {
                    unsafe { Err(Some(Weak::<_, R>::clone_from_raw(before))) }
                }
            }
        }
    }

    /// Loads the pointer and returns a [`Weak`] or [`None`] if it is null.
    pub fn load(&self, order: Ordering) -> Option<Weak<T, R>> {
        with_critical_section::<R, _, _>(|| {
            let ptr = self.ptr.load(order);
            if ptr.is_null() {
                None
            } else {
                unsafe { Some(Weak::clone_from_raw(ptr)) }
            }
        })
    }

    /// Stores `new`'s pointer (or [`None`]) into `self`.
    pub fn store<N: Shared<T>>(&self, new: Option<&N>, order: Ordering) {
        let ptr: *const T = new.map_or(null(), N::as_ptr);
        if !ptr.is_null() {
            unsafe {
                Weak::<_, R>::increment_weak_count(ptr);
            }
        }
        let before = self.ptr.swap(ptr.cast_mut(), order);
        if !before.is_null() {
            unsafe {
                drop(Weak::<_, R>::from_raw(before));
            }
        }
    }

    /// Returns a [`Strong`] (an [`Arc`] or a [`Snapshot`]) if the strong count is at least one.
    /// Analogous to [`std::sync::Weak::upgrade`].
    pub fn upgrade<V: Strong<T>>(&self, order: Ordering) -> Option<V> {
        with_critical_section::<R, _, _>(|| {
            let ptr = self.ptr.load(order);
            if ptr.is_null() {
                None
            } else {
                unsafe { V::try_clone_from_raw(ptr) }
            }
        })
    }
}

impl<T: 'static, R: Protect + Retire> Clone for AtomicWeak<T, R> {
    fn clone(&self) -> Self {
        let ptr = with_critical_section::<R, _, _>(|| {
            let ptr = self.ptr.load(SeqCst);
            if !ptr.is_null() {
                unsafe {
                    Weak::<_, R>::increment_weak_count(ptr);
                }
            }
            ptr
        });
        Self {
            ptr: AtomicPtr::new(ptr),
            phantom_r: PhantomData,
        }
    }
}

impl<T: 'static> Default for AtomicWeak<T, StandardReclaimer> {
    fn default() -> Self {
        Self {
            ptr: AtomicPtr::default(),
            phantom_r: PhantomData,
        }
    }
}

impl<T: 'static, R: Protect + Retire> Drop for AtomicWeak<T, R> {
    fn drop(&mut self) {
        let ptr = self.ptr.load(SeqCst);
        if !ptr.is_null() {
            unsafe {
                drop(Weak::<_, R>::from_raw(ptr));
            }
        }
    }
}

impl<T: 'static, R: Protect + ProtectPtr + Retire> From<&Snapshot<T, R>> for AtomicArc<T, R> {
    fn from(value: &Snapshot<T, R>) -> Self {
        unsafe {
            let inner = Snapshot::as_ptr(value) as *const ArcInner<T>;
            (*inner).increment_strong_count();
            Self {
                ptr: AtomicPtr::new(inner as *mut T),
                phantom: PhantomData,
                phantom_r: PhantomData,
            }
        }
    }
}

impl<T: 'static, R: Protect + ProtectPtr + Retire> From<&Snapshot<T, R>> for AtomicWeak<T, R> {
    fn from(value: &Snapshot<T, R>) -> Self {
        unsafe {
            let inner = Snapshot::as_ptr(value) as *const ArcInner<T>;
            (*inner).increment_weak_count();
            Self {
                ptr: AtomicPtr::new(inner as *mut T),
                phantom_r: PhantomData,
            }
        }
    }
}

fn with_critical_section<R: Protect, V, F: Fn() -> V>(f: F) -> V {
    R::begin_critical_section();
    let result = f();
    R::end_critical_section();
    result
}

/// A marker trait for pointers that prevent deallocation of an object. Implemented by [`Arc`] and
/// [`Snapshot`], but not by [`Weak`].
pub trait Strong<T>: Shared<T> + TryCloneFromRaw<T> {}

impl<T, X> Strong<T> for X where X: Shared<T> + TryCloneFromRaw<T> {}

/// A marker trait for shared pointers. Implemented by [`Arc`], [`Snapshot`], and [`Weak`].
pub trait Shared<T>: AsPtr<T> + CloneFromRaw<T> {}

impl<T, X> Shared<T> for X where X: AsPtr<T> + CloneFromRaw<T> {}

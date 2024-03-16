use crate::shared_ptrs::{ArcInner, AsPtr, CloneFromRaw, TryCloneFromRaw};
use crate::smr::drc::{Protect, ProtectPtr, ProvideGlobal, Retire};
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
pub struct AtomicArc<'a, T: 'a, R: Protect + Retire = StandardReclaimer> {
    ptr: AtomicPtr<T>,
    phantom: PhantomData<T>,
    reclaimer: &'a R,
}

impl<T: 'static> AtomicArc<'static, T, StandardReclaimer> {
    /// Similar to [`Arc::new`], but [`None`] is a valid input, in which case the `AtomicArc` will
    /// be empty to represent a null pointer.
    ///
    /// To create an `AtomicArc` from an existing [`Arc`], use `from`.
    pub fn new(data: Option<T>) -> Self {
        Self::new_in(data, StandardReclaimer::get_global())
    }
}

impl<'a, T: 'a, R: Protect + Retire> AtomicArc<'a, T, R> {
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
        match with_critical_section(self.reclaimer, || {
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
        with_critical_section(self.reclaimer, || {
            let ptr = self.ptr.load(order);
            if ptr.is_null() {
                None
            } else {
                unsafe { Some(V::clone_from_raw(ptr)) }
            }
        })
    }

    /// Analogous to [`Arc::new_in`].
    pub fn new_in(data: Option<T>, reclaimer: &'a R) -> Self {
        let ptr = data.map_or(null(), |x| Arc::into_raw(Arc::new_in(x, reclaimer)));
        Self {
            ptr: AtomicPtr::new(ptr.cast_mut()),
            phantom: PhantomData,
            reclaimer,
        }
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

impl<'a, T: 'a, R: Protect + Retire> Clone for AtomicArc<'a, T, R> {
    fn clone(&self) -> Self {
        let ptr = with_critical_section(self.reclaimer, || {
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
            reclaimer: self.reclaimer,
        }
    }
}

impl<T: 'static> Default for AtomicArc<'static, T, StandardReclaimer> {
    fn default() -> Self {
        Self {
            ptr: AtomicPtr::default(),
            phantom: PhantomData,
            reclaimer: StandardReclaimer::get_global(),
        }
    }
}

impl<'a, T: 'a, R: Protect + Retire> Drop for AtomicArc<'a, T, R> {
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
pub struct AtomicWeak<'a, T: 'a, R: Protect + Retire = StandardReclaimer> {
    ptr: AtomicPtr<T>,
    reclaimer: &'a R,
}

impl<'a, T: 'a, R: Protect + Retire> AtomicWeak<'a, T, R> {
    /// See [`AtomicArc::compare_exchange`]. This method behaves similarly, except that the return
    /// type for the failure case cannot be specified by the caller; it must be a [`Weak`].
    pub fn compare_exchange<C, N>(
        &self,
        current: Option<&C>,
        new: Option<&N>,
        success: Ordering,
        failure: Ordering,
    ) -> Result<(), Option<Weak<'a, T, R>>>
    where
        C: Shared<T>,
        N: Shared<T>,
    {
        let c: *const T = current.map_or(null(), C::as_ptr);
        let n: *const T = new.map_or(null(), N::as_ptr);
        match with_critical_section(self.reclaimer, || {
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
    pub fn load(&self, order: Ordering) -> Option<Weak<'a, T, R>> {
        with_critical_section(self.reclaimer, || {
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
        with_critical_section(self.reclaimer, || {
            let ptr = self.ptr.load(order);
            if ptr.is_null() {
                None
            } else {
                V::try_clone_from_raw(ptr)
            }
        })
    }
}

impl<'a, T: 'a, R: Protect + Retire> Clone for AtomicWeak<'a, T, R> {
    fn clone(&self) -> Self {
        let ptr = with_critical_section(self.reclaimer, || {
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
            reclaimer: self.reclaimer,
        }
    }
}

impl<T: 'static> Default for AtomicWeak<'static, T, StandardReclaimer> {
    fn default() -> Self {
        Self {
            ptr: AtomicPtr::default(),
            reclaimer: StandardReclaimer::get_global(),
        }
    }
}

impl<'a, T: 'a, R: Protect + Retire> Drop for AtomicWeak<'a, T, R> {
    fn drop(&mut self) {
        let ptr = self.ptr.load(SeqCst);
        if !ptr.is_null() {
            unsafe {
                drop(Weak::<_, R>::from_raw(ptr));
            }
        }
    }
}

impl<'a, T: 'a, R: Protect + ProtectPtr + Retire> From<&Snapshot<'a, T, R>>
    for AtomicArc<'a, T, R>
{
    fn from(value: &Snapshot<T, R>) -> Self {
        unsafe {
            let inner = Snapshot::as_ptr(value) as *const ArcInner<T, R>;
            (*inner).increment_strong_count();
            Self {
                ptr: AtomicPtr::new(inner as *mut T),
                phantom: PhantomData,
                reclaimer: (*inner).reclaimer(),
            }
        }
    }
}

impl<'a, T: 'a, R: Protect + ProtectPtr + Retire> From<&Snapshot<'a, T, R>>
    for AtomicWeak<'a, T, R>
{
    fn from(value: &Snapshot<T, R>) -> Self {
        unsafe {
            let inner = Snapshot::as_ptr(value) as *const ArcInner<T, R>;
            (*inner).increment_weak_count();
            Self {
                ptr: AtomicPtr::new(inner as *mut T),
                reclaimer: (*inner).reclaimer(),
            }
        }
    }
}

fn with_critical_section<'f, 'a: 'f, R: Protect + Retire, V, F: Fn() -> V>(m: &'a R, f: F) -> V {
    m.begin_critical_section();
    let result = f();
    m.end_critical_section();
    result
}

/// A marker trait for pointers that prevent deallocation of an object. Implemented by [`Arc`] and
/// [`Snapshot`], but not by [`Weak`].
pub trait Strong<T>: Shared<T> + TryCloneFromRaw<T> {}

impl<T, X> Strong<T> for X where X: Shared<T> + TryCloneFromRaw<T> {}

/// A marker trait for shared pointers. Implemented by [`Arc`], [`Snapshot`], and [`Weak`].
pub trait Shared<T>: AsPtr<T> + CloneFromRaw<T> {}

impl<T, X> Shared<T> for X where X: AsPtr<T> + CloneFromRaw<T> {}

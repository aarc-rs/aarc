use crate::smart_ptrs::{CloneFromRaw, SmartPtr, StrongPtr};
use crate::smr::drc::{Protect, ProtectPtr, Retire};
use crate::smr::standard_reclaimer::StandardReclaimer;
use crate::{Arc, AsPtr, Snapshot, Weak};
use std::marker::PhantomData;
use std::ptr;
use std::ptr::{null, null_mut};
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed};

/// An [`Arc`] with an atomically updatable pointer.
///
/// An `AtomicArc` contributes to the strong count of the pointed-to allocation (if any). It does
/// not implement [`Deref`][`std::ops::Deref`], so a method like `load` must be used to obtain an
/// [`Arc`] or a [`Snapshot`] through which the data can be read.
///
/// Unlike [`Arc`], `AtomicArc` can store [`None`]. Notice that a hypothetical
/// `Option<AtomicArc<T>>` would be pointless, as the variable would lose its atomicity.
/// Thus, `AtomicArc` uses the
/// [null pointer optimization](https://doc.rust-lang.org/std/option/index.html#representation)
/// to intrinsically support [`Option`].
///
/// All methods use load-acquire, store-release semantics.
///
/// # Examples
/// ```
/// use aarc::{Arc, AtomicArc, Snapshot};
///
/// let atomic = AtomicArc::new(Some(53)); // +1 strong count on val 53
///
/// let snapshot53 = atomic.load::<Snapshot<_>>(); // snapshot doesn't affect counts
/// assert_eq!(*snapshot53.unwrap(), 53);
///
/// let arc53 = atomic.load::<Arc<_>>().unwrap(); // +1 strong count on val 53
/// assert_eq!(*arc53, 53);
/// assert_eq!(Arc::strong_count(&arc53), 2);
///
/// let arc75 = Arc::new(75); // +1 strong count on val 75
/// atomic.store(Some(&arc75)); // +1 strong on 75; -1 strong on 53 does not occur immediately
/// assert_eq!(Arc::strong_count(&arc53), 2);
/// assert_eq!(Arc::strong_count(&arc75), 2);
///
/// let snapshot75 = atomic.load::<Snapshot<_>>();
/// assert_eq!(*snapshot75.unwrap(), 75);
/// ```
pub struct AtomicArc<T: 'static, R: Protect + Retire = StandardReclaimer> {
    ptr: AtomicPtr<T>,
    phantom: PhantomData<T>,
    phantom_r: PhantomData<R>,
}

impl<T: 'static> AtomicArc<T, StandardReclaimer> {
    /// Similar to [`Arc::new`], but [`None`] is a valid input, in which case the `AtomicArc` will
    /// store a null pointer.
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
    /// If `self` and `current` point to the same allocation, `new`'s pointer will be stored into
    /// `self` and the result will be an empty [`Ok`]. Otherwise, an [`Err`] containing the
    /// previous value will be returned.
    pub fn compare_exchange<C, N, V>(
        &self,
        current: Option<&C>,
        new: Option<&N>,
    ) -> Result<(), Option<V>>
    where
        C: SmartPtr<T>,
        N: StrongPtr<T>,
        V: SmartPtr<T>,
    {
        let c: *const T = current.map_or(null(), C::as_ptr);
        let n: *const T = new.map_or(null(), N::as_ptr);
        let mut to_drop: *mut T = null_mut();
        let result = with_critical_section::<R, _, _>(|| {
            match self
                .ptr
                .compare_exchange(c.cast_mut(), n.cast_mut(), AcqRel, Acquire)
            {
                Ok(before) => unsafe {
                    if !n.is_null() && !ptr::eq(n, before) {
                        Arc::<_, R>::increment_strong_count(n);
                        to_drop = before;
                    }
                    Ok(())
                },
                Err(before) => unsafe {
                    if before.is_null() {
                        Err(None)
                    } else {
                        Err(Some(V::clone_from_raw(before)))
                    }
                },
            }
        });
        if !to_drop.is_null() {
            unsafe {
                drop(Arc::<_, R>::from_raw(to_drop));
            }
        }
        result
    }

    /// Loads the pointer and returns the desired type (`Arc` or `Snapshot`), or [`None`] if it is
    /// null.
    pub fn load<V: SmartPtr<T>>(&self) -> Option<V> {
        with_critical_section::<R, _, _>(|| unsafe {
            let ptr = self.ptr.load(Acquire);
            if ptr.is_null() {
                None
            } else {
                Some(V::clone_from_raw(ptr))
            }
        })
    }

    /// Stores `new`'s pointer (or [`None`]) into `self`.
    pub fn store<N: StrongPtr<T>>(&self, new: Option<&N>) {
        let ptr: *const T = new.map_or(null(), N::as_ptr);
        if !ptr.is_null() {
            unsafe {
                Arc::<_, R>::increment_strong_count(ptr);
            }
        }
        let before = self.ptr.swap(ptr.cast_mut(), AcqRel);
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
            let ptr = self.ptr.load(Acquire);
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
        let ptr = self.ptr.load(Relaxed);
        if !ptr.is_null() {
            unsafe {
                drop(Arc::<_, R>::from_raw(ptr));
            }
        }
    }
}

unsafe impl<T: 'static + Send + Sync, R: Protect + Retire> Send for AtomicArc<T, R> {}

unsafe impl<T: 'static + Send + Sync, R: Protect + Retire> Sync for AtomicArc<T, R> {}

/// A [`Weak`] with an atomically updatable pointer.
///
/// `AtomicWeak` is similar to [`AtomicArc`], but it contributes to the weak count instead of the
/// strong count.
///
/// # Examples
/// ```
/// use aarc::{Arc, AtomicWeak, Snapshot};
///
/// let arc1 = Arc::new(53); // +1 strong count
///
/// let atomic = AtomicWeak::default();
/// atomic.store(Some(&arc1)); // +1 weak count
///
/// let snapshot = atomic.upgrade::<Snapshot<_>>(); // snapshot doesn't affect counts
/// assert_eq!(*snapshot.unwrap(), 53);
///
/// let weak = atomic.load().unwrap(); // +1 weak count
/// let arc2 = weak.upgrade::<Arc<_>>().unwrap(); // +1 strong count
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
    ) -> Result<(), Option<Weak<T, R>>>
    where
        C: SmartPtr<T>,
        N: SmartPtr<T>,
    {
        let c: *const T = current.map_or(null(), C::as_ptr);
        let n: *const T = new.map_or(null(), N::as_ptr);
        let mut to_drop: *mut T = null_mut();
        let result = with_critical_section::<R, _, _>(|| {
            match self
                .ptr
                .compare_exchange(c.cast_mut(), n.cast_mut(), AcqRel, Acquire)
            {
                Ok(before) => unsafe {
                    if !n.is_null() && !ptr::eq(n, before) {
                        Weak::<_, R>::increment_weak_count(n);
                        to_drop = before;
                    }
                    Ok(())
                },
                Err(before) => unsafe {
                    if before.is_null() {
                        Err(None)
                    } else {
                        Err(Some(Weak::<_, R>::clone_from_raw(before)))
                    }
                },
            }
        });
        if !to_drop.is_null() {
            unsafe {
                drop(Weak::<_, R>::from_raw(to_drop));
            }
        }
        result
    }

    /// Loads the pointer and returns a [`Weak`], or [`None`] if it is null.
    pub fn load(&self) -> Option<Weak<T, R>> {
        with_critical_section::<R, _, _>(|| {
            let ptr = self.ptr.load(Acquire);
            if ptr.is_null() {
                None
            } else {
                unsafe { Some(Weak::clone_from_raw(ptr)) }
            }
        })
    }

    /// Stores `new`'s pointer (or [`None`]) into `self`.
    pub fn store<N: SmartPtr<T>>(&self, new: Option<&N>) {
        let ptr: *const T = new.map_or(null(), N::as_ptr);
        if !ptr.is_null() {
            unsafe {
                Weak::<_, R>::increment_weak_count(ptr);
            }
        }
        let before = self.ptr.swap(ptr.cast_mut(), AcqRel);
        if !before.is_null() {
            unsafe {
                drop(Weak::<_, R>::from_raw(before));
            }
        }
    }

    /// Returns a [`StrongPtr`] (an [`Arc`] or a [`Snapshot`]) if the strong count is at least one.
    /// Analogous to [`std::sync::Weak::upgrade`].
    pub fn upgrade<V: StrongPtr<T>>(&self) -> Option<V> {
        with_critical_section::<R, _, _>(|| {
            let ptr = self.ptr.load(Acquire);
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
            let ptr = self.ptr.load(Acquire);
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
        let ptr = self.ptr.load(Relaxed);
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
            let ptr = Snapshot::as_ptr(value);
            Arc::<_, R>::increment_strong_count(ptr);
            Self {
                ptr: AtomicPtr::new(ptr.cast_mut()),
                phantom: PhantomData,
                phantom_r: PhantomData,
            }
        }
    }
}

impl<T: 'static, R: Protect + ProtectPtr + Retire> From<&Snapshot<T, R>> for AtomicWeak<T, R> {
    fn from(value: &Snapshot<T, R>) -> Self {
        unsafe {
            let ptr = Snapshot::as_ptr(value);
            Weak::<_, R>::increment_weak_count(ptr);
            Self {
                ptr: AtomicPtr::new(ptr.cast_mut()),
                phantom_r: PhantomData,
            }
        }
    }
}

impl<T: 'static, R: Protect + ProtectPtr + Retire> From<&Arc<T, R>> for AtomicArc<T, R> {
    fn from(value: &Arc<T, R>) -> Self {
        unsafe {
            let ptr = Arc::as_ptr(value);
            Arc::<_, R>::increment_strong_count(ptr);
            Self {
                ptr: AtomicPtr::new(ptr.cast_mut()),
                phantom: PhantomData,
                phantom_r: PhantomData,
            }
        }
    }
}

impl<T: 'static, R: Protect + ProtectPtr + Retire> From<&Arc<T, R>> for AtomicWeak<T, R> {
    fn from(value: &Arc<T, R>) -> Self {
        unsafe {
            let ptr = Arc::as_ptr(value);
            Weak::<_, R>::increment_weak_count(ptr);
            Self {
                ptr: AtomicPtr::new(ptr.cast_mut()),
                phantom_r: PhantomData,
            }
        }
    }
}

unsafe impl<T: 'static + Send + Sync, R: Protect + Retire> Send for AtomicWeak<T, R> {}

unsafe impl<T: 'static + Send + Sync, R: Protect + Retire> Sync for AtomicWeak<T, R> {}

fn with_critical_section<R: Protect, V, F: FnMut() -> V>(mut f: F) -> V {
    R::begin_critical_section();
    let result = f();
    R::end_critical_section();
    result
}

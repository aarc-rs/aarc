use std::marker::PhantomData;
use std::ptr;
use std::ptr::{null, null_mut, NonNull};
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::Ordering::{Relaxed, SeqCst};

use fast_smr::smr::{load, protect};

use crate::smart_ptrs::{Arc, AsPtr, Guard, Weak};
use crate::StrongPtr;

/// An [`Arc`] with an atomically updatable pointer.
///
/// Usage notes:
/// * An `AtomicArc` can intrinsically store `None` (a hypothetical `Option<AtomicArc<T>>` would
///   no longer be atomic).
/// * An `AtomicArc` contributes to the strong count of the pointed-to allocation, if any. However,
///   it does not implement `Deref`, so methods like `load` must be used to obtain a [`Guard`]
///   through which the data can be accessed.
/// * `T` must be `Sized` for compatibility with `AtomicPtr`. This may be relaxed in the future.
/// * When an `AtomicArc` is updated or dropped, the strong count of the previously pointed-to
///   object may not be immediately decremented. Thus:
///     * `T` must be `'static` to support delayed deallocations.
///     * The value returned by `strong_count` will likely be an overestimate.
///
/// # Examples
/// ```
/// use aarc::{Arc, AtomicArc, Guard, RefCount};
///
/// let atomic = AtomicArc::new(53);
///
/// let guard = atomic.load().unwrap(); // guard doesn't affect strong count
/// assert_eq!(*guard, 53);
///
/// let arc = Arc::from(&guard);
/// assert_eq!(arc.strong_count(), 2);
///
/// assert_eq!(*arc, *guard);
/// ```
#[derive(Default)]
pub struct AtomicArc<T: 'static> {
    ptr: AtomicPtr<T>,
    phantom: PhantomData<T>,
}

impl<T: 'static> AtomicArc<T> {
    /// Similar to [`Arc::new`], but `None` is a valid input, in which case the `AtomicArc` will
    /// store a null pointer.
    ///
    /// To create an `AtomicArc` from an existing `Arc`, use `from`.
    pub fn new<D: Into<Option<T>>>(data: D) -> Self {
        let ptr = data.into().map_or(null(), |x| Arc::into_raw(Arc::new(x)));
        Self {
            ptr: AtomicPtr::new(ptr.cast_mut()),
            phantom: PhantomData,
        }
    }

    /// If `self` and `current` point to the same object, new’s pointer will be stored into self
    /// and the result will be an empty `Ok`. Otherwise, a `load` occurs, and an `Err` containing
    /// a [`Guard`] will be returned.
    pub fn compare_exchange<N: AsPtr<Target = T> + StrongPtr>(
        &self,
        current: *const T,
        new: Option<&N>,
    ) -> Result<(), Option<Guard<T>>> {
        let c = current.cast_mut();
        let n = new.map_or(null(), N::as_ptr).cast_mut();
        match self.ptr.compare_exchange(c, n, SeqCst, SeqCst) {
            Ok(before) => unsafe {
                Self::after_swap(n, before);
                Ok(())
            },
            Err(actual) => {
                let mut opt = None;
                if let Some(ptr) = NonNull::new(actual) {
                    if let Some(guard) = protect(&self.ptr, ptr) {
                        opt = Some(Guard { guard })
                    }
                }
                Err(opt)
            }
        }
    }

    /// Loads a [`Guard`], which allows the pointed-to value to be accessed. `None` indicates that
    /// the inner atomic pointer is null.
    pub fn load(&self) -> Option<Guard<T>> {
        let guard = load(&self.ptr)?;
        Some(Guard { guard })
    }

    /// Stores `new`'s pointer (or `None`) into `self`.
    pub fn store<N: AsPtr<Target = T> + StrongPtr>(&self, new: Option<&N>) {
        // TODO: rework this method to possibly take ownership of new (avoid increment).
        let n = new.map_or(null(), N::as_ptr);
        let before = self.ptr.swap(n.cast_mut(), SeqCst);
        unsafe {
            Self::after_swap(n, before);
        }
    }

    unsafe fn after_swap(new: *const T, before: *const T) {
        if !ptr::eq(new, before) {
            if !new.is_null() {
                Arc::increment_strong_count(new);
            }
            if !before.is_null() {
                drop(Arc::from_raw(before));
            }
        }
    }
}

impl<T: 'static> Clone for AtomicArc<T> {
    fn clone(&self) -> Self {
        let ptr = if let Some(guard) = self.load() {
            unsafe {
                Arc::increment_strong_count(guard.as_ptr());
            }
            guard.as_ptr().cast_mut()
        } else {
            null_mut()
        };
        Self {
            ptr: AtomicPtr::new(ptr),
            phantom: PhantomData,
        }
    }
}

impl<T: 'static> Drop for AtomicArc<T> {
    fn drop(&mut self) {
        if let Some(ptr) = NonNull::new(self.ptr.load(Relaxed)) {
            unsafe {
                drop(Arc::from_raw(ptr.as_ptr()));
            }
        }
    }
}

unsafe impl<T: 'static + Send + Sync> Send for AtomicArc<T> {}

unsafe impl<T: 'static + Send + Sync> Sync for AtomicArc<T> {}

/// A [`Weak`] with an atomically updatable pointer.
///
/// See [`AtomicArc`] for usage notes. `AtomicWeak` differs only in that it contributes to the weak
/// count instead of the strong count.
///
/// # Examples
/// ```
/// use aarc::{Arc, AtomicWeak, RefCount, Weak};
///
/// let arc = Arc::new(53);
///
/// let atomic = AtomicWeak::from(&arc); // +1 weak count
///
/// let guard = atomic.load().unwrap();
///
/// assert_eq!(*arc, *guard);
/// assert_eq!(arc.weak_count(), 1);
/// ```
#[derive(Default)]
pub struct AtomicWeak<T: 'static> {
    ptr: AtomicPtr<T>,
}

impl<T: 'static> AtomicWeak<T> {
    /// If `self` and `current` point to the same object, new’s pointer will be stored into self
    /// and the result will be an empty `Ok`. Otherwise, a load will be attempted and a
    /// [`Guard`] will be returned if possible. See `load`.
    pub fn compare_exchange<N: AsPtr<Target = T>>(
        &self,
        current: *const T,
        new: Option<&N>,
    ) -> Result<(), Option<Guard<T>>> {
        let c = current.cast_mut();
        let n = new.map_or(null(), N::as_ptr).cast_mut();
        match self.ptr.compare_exchange(c, n, SeqCst, SeqCst) {
            Ok(before) => unsafe {
                Self::after_swap(n, before);
                Ok(())
            },
            Err(actual) => unsafe {
                let mut opt = None;
                if let Some(ptr) = NonNull::new(actual) {
                    if let Some(guard) = protect(&self.ptr, ptr) {
                        opt = (Arc::strong_count_raw(guard.as_ptr()) > 0).then_some(Guard { guard })
                    }
                }
                Err(opt)
            },
        }
    }

    /// Attempts to load a [`Guard`]. This method differs from the one on `AtomicArc` in that
    /// `None` may indicate one of two things:
    /// * The `AtomicWeak` is indeed not pointing to anything (null pointer).
    /// * The pointer is not null, but the strong count is 0, so a `Guard` cannot be loaded.
    ///
    /// There is currently no way for the user to differentiate between the two cases (this may
    /// change in the future).
    pub fn load(&self) -> Option<Guard<T>> {
        let guard = load(&self.ptr)?;
        unsafe { (Arc::strong_count_raw(guard.as_ptr()) > 0).then_some(Guard { guard }) }
    }

    /// Stores `new`'s pointer (or `None`) into `self`.
    pub fn store<N: AsPtr<Target = T>>(&self, new: Option<&N>) {
        let n = new.map_or(null(), N::as_ptr);
        let before = self.ptr.swap(n.cast_mut(), SeqCst);
        unsafe {
            Self::after_swap(n, before);
        }
    }

    unsafe fn after_swap(new: *const T, before: *const T) {
        if !ptr::eq(new, before) {
            if !new.is_null() {
                Weak::increment_weak_count(new);
            }
            if !before.is_null() {
                drop(Weak::from_raw(before));
            }
        }
    }
}

impl<T: 'static> Clone for AtomicWeak<T> {
    fn clone(&self) -> Self {
        let ptr = if let Some(guard) = self.load() {
            unsafe {
                Weak::increment_weak_count(guard.as_ptr());
            }
            guard.as_ptr().cast_mut()
        } else {
            null_mut()
        };
        Self {
            ptr: AtomicPtr::new(ptr),
        }
    }
}

impl<T: 'static> Drop for AtomicWeak<T> {
    fn drop(&mut self) {
        if let Some(ptr) = NonNull::new(self.ptr.load(Relaxed)) {
            unsafe {
                drop(Weak::from_raw(ptr.as_ptr()));
            }
        }
    }
}

impl<T: 'static, P: AsPtr<Target = T> + StrongPtr> From<&P> for AtomicArc<T> {
    fn from(value: &P) -> Self {
        unsafe {
            let ptr = P::as_ptr(value);
            Arc::increment_strong_count(ptr);
            Self {
                ptr: AtomicPtr::new(ptr.cast_mut()),
                phantom: PhantomData,
            }
        }
    }
}

impl<T: 'static, P: AsPtr<Target = T>> From<&P> for AtomicWeak<T> {
    fn from(value: &P) -> Self {
        unsafe {
            let ptr = P::as_ptr(value);
            Weak::increment_weak_count(ptr);
            Self {
                ptr: AtomicPtr::new(ptr.cast_mut()),
            }
        }
    }
}

unsafe impl<T: 'static + Send + Sync> Send for AtomicWeak<T> {}

unsafe impl<T: 'static + Send + Sync> Sync for AtomicWeak<T> {}

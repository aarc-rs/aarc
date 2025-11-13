use std::marker::PhantomData;
use std::ptr::{eq, null, null_mut, NonNull};
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::Ordering::SeqCst;

use crate::smart_ptrs::{find_inner_ptr, ArcInner, Guard, CTX};
use crate::Arc;

/// An [`Arc`] with an atomically updatable pointer.
///
/// Usage notes:
/// * An `AtomicArc` can intrinsically store `None` (a hypothetical `Option<AtomicArc<T>>` would
///   no longer be atomic).
/// * An `AtomicArc` contributes to the strong count of the pointed-to allocation, if any. However,
///   it does not implement `Deref`, so methods like `load` must be used to obtain a [`Guard`]
///   through which the data can be accessed.
/// * `T` must be `Sized`. This may be relaxed in the future.
/// * When an `AtomicArc` is updated or dropped, the strong count of the previously pointed-to
///   object may not be immediately decremented. Thus:
///     * `T` must be `'static` to support delayed deallocations.
///     * The value returned by `ref_count` may be an overestimate.
///
/// # Examples
/// ```
/// use aarc::{Arc, AtomicArc, Guard};
///
/// // ref count: 1
/// let x = Arc::new(53);
/// assert_eq!(Arc::ref_count(&x), 1);
///
/// // ref count: 2
/// let atomic = AtomicArc::new(0);
/// atomic.store(Some(&x));
/// assert_eq!(Arc::ref_count(&x), 2);
///
/// // guard doesn't affect the ref count
/// let guard = atomic.load().unwrap();
/// assert_eq!(Arc::ref_count(&x), 2);
///
/// // both the `Arc` and the `Guard` point to the same block
/// assert_eq!(*guard, 53);
/// assert_eq!(*guard, *x);
/// ```
#[derive(Default)]
pub struct AtomicArc<T: 'static> {
    ptr: AtomicPtr<ArcInner<T>>,
    phantom: PhantomData<ArcInner<T>>,
}

impl<T: 'static> AtomicArc<T> {
    /// Similar to [`Arc::new`], but `None` is a valid input, in which case the `AtomicArc` will
    /// store a null pointer.
    ///
    /// To create an `AtomicArc` from an existing `Arc`, use `from`.
    pub fn new<D: Into<Option<T>>>(data: D) -> Self {
        let ptr = data.into().map_or(null_mut(), ArcInner::new);
        Self {
            ptr: AtomicPtr::new(ptr),
            phantom: PhantomData,
        }
    }

    /// Loads a [`Guard`], which allows the pointed-to value to be accessed. `None` indicates that
    /// the inner atomic pointer is null.
    pub fn load(&self) -> Option<Guard<'static, T>> {
        let guard = CTX.with_borrow(|ctx| ctx.load(&self.ptr, 1))?;
        Some(Guard { guard })
    }

    /// Stores `new`'s pointer (or `None`) into `self` and returns the previously-stored `Arc`.
    pub fn swap<N: Into<NonNull<T>>>(&self, new: Option<N>) -> Option<Arc<T>> {
        unsafe {
            let n = new.map_or(null_mut(), |n| find_inner_ptr(n.into().as_ptr()).cast_mut());
            if !n.is_null() {
                ArcInner::increment(n);
            }
            let before = NonNull::new(self.ptr.swap(n, SeqCst))?;
            Some(Arc {
                ptr: before,
                phantom: PhantomData,
            })
        }
    }

    /// Stores `new`'s pointer (or `None`) into `self`. Equivalent to `swap`, but discards the result.
    pub fn store<N: Into<NonNull<T>>>(&self, new: Option<N>) {
        _ = self.swap(new)
    }
}

/// A trait for implementations of `compare_exchange` on `AtomicArc`.
///
/// If `self` and `current` point to the same object, new’s pointer will be stored into self
/// and the result will be an empty `Ok`. Otherwise, a `load` occurs, and an `Err` containing
/// a [`Guard`] will be returned.
pub trait CompareExchange<T, N> {
    fn compare_exchange<C: Into<NonNull<T>>>(
        &self,
        current: Option<C>,
        new: Option<N>,
    ) -> Result<(), Option<Guard<'static, T>>>;
}

impl<T: 'static> CompareExchange<T, &Guard<'static, T>> for AtomicArc<T> {
    fn compare_exchange<C: Into<NonNull<T>>>(
        &self,
        current: Option<C>,
        new: Option<&Guard<'static, T>>,
    ) -> Result<(), Option<Guard<'static, T>>> {
        unsafe {
            let c = current.map_or(null_mut(), |c| find_inner_ptr(c.into().as_ptr()).cast_mut());
            let n = new.map_or(null(), Guard::inner_ptr).cast_mut();
            match self.ptr.compare_exchange(c, n, SeqCst, SeqCst) {
                Ok(before) => {
                    if !eq(before, n) {
                        if !n.is_null() {
                            ArcInner::increment(n);
                        }
                        if !before.is_null() {
                            ArcInner::delayed_decrement(before);
                        }
                    }
                    Ok(())
                }
                Err(actual) => {
                    if let Some(ptr) = NonNull::new(actual) {
                        let mut opt = None;
                        let loaded = CTX.with_borrow(|ctx| ctx.protect(&self.ptr, ptr, 1));
                        if let Some(guard) = loaded {
                            opt = Some(Guard { guard })
                        }
                        Err(opt)
                    } else {
                        Err(None)
                    }
                }
            }
        }
    }
}

impl<T: 'static> CompareExchange<T, &Arc<T>> for AtomicArc<T> {
    fn compare_exchange<C: Into<NonNull<T>>>(
        &self,
        current: Option<C>,
        new: Option<&Arc<T>>,
    ) -> Result<(), Option<Guard<'static, T>>> {
        let g = new.map(Guard::from);
        CompareExchange::compare_exchange(self, current, g.as_ref())
    }
}

impl<T: 'static> Clone for AtomicArc<T> {
    fn clone(&self) -> Self {
        let ptr = if let Some(guard) = self.load() {
            unsafe {
                let ptr = guard.guard.as_ptr();
                _ = (*ptr).ref_count.fetch_add(1, SeqCst);
                ptr
            }
        } else {
            null_mut()
        };
        Self {
            ptr: AtomicPtr::new(ptr.cast_mut()),
            phantom: PhantomData,
        }
    }
}

impl<T: 'static> Drop for AtomicArc<T> {
    fn drop(&mut self) {
        if let Some(ptr) = NonNull::new(self.ptr.load(SeqCst)) {
            unsafe {
                ArcInner::delayed_decrement(ptr.as_ptr());
            }
        }
    }
}

unsafe impl<T: 'static + Send + Sync> Send for AtomicArc<T> {}

unsafe impl<T: 'static + Send + Sync> Sync for AtomicArc<T> {}

impl<T: 'static, P: Into<NonNull<T>>> From<P> for AtomicArc<T> {
    fn from(value: P) -> Self {
        unsafe {
            let inner_ptr = find_inner_ptr(value.into().as_ptr());
            _ = (*inner_ptr).ref_count.fetch_add(1, SeqCst);
            Self {
                ptr: AtomicPtr::new(inner_ptr.cast_mut()),
                phantom: PhantomData,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Arc, AtomicArc, CompareExchange};

    #[test]
    fn test_new_with_value() {
        let atomic = AtomicArc::new(42);
        let guard = atomic.load().unwrap();
        assert_eq!(*guard, 42);
    }

    #[test]
    fn test_new_with_none() {
        let atomic: AtomicArc<i32> = AtomicArc::new(None);
        assert!(atomic.load().is_none());
    }

    #[test]
    fn test_swap() {
        let atomic = AtomicArc::new(10);
        let arc = Arc::new(20);

        let old = atomic.swap(Some(&arc));
        assert!(old.is_some());
        assert_eq!(*old.unwrap(), 10);

        let guard = atomic.load().unwrap();
        assert_eq!(*guard, 20);
    }

    #[test]
    fn test_swap_none() {
        let atomic = AtomicArc::new(10);
        let old = atomic.swap::<&Arc<i32>>(None);

        assert!(old.is_some());
        assert_eq!(*old.unwrap(), 10);
        assert!(atomic.load().is_none());
    }

    #[test]
    fn test_clone() {
        let atomic = AtomicArc::new(42);
        let cloned = atomic.clone();

        let guard1 = atomic.load().unwrap();
        let guard2 = cloned.load().unwrap();

        assert_eq!(*guard1, 42);
        assert_eq!(*guard2, 42);
    }

    #[test]
    fn test_clone_none() {
        let atomic: AtomicArc<i32> = AtomicArc::new(None);
        let cloned = atomic.clone();

        assert!(atomic.load().is_none());
        assert!(cloned.load().is_none());
    }

    #[test]
    fn test_compare_exchange_success_with_arc() {
        let arc1 = Arc::new(10);
        let arc2 = Arc::new(20);
        let atomic = AtomicArc::new(10);
        atomic.store(Some(&arc1));

        let result = atomic.compare_exchange(Some(&arc1), Some(&arc2));
        assert!(result.is_ok());

        let guard = atomic.load().unwrap();
        assert_eq!(*guard, 20);
    }

    #[test]
    fn test_compare_exchange_failure_with_arc() {
        let arc1 = Arc::new(10);
        let arc2 = Arc::new(20);
        let arc3 = Arc::new(30);
        let atomic = AtomicArc::new(10);
        atomic.store(Some(&arc1));

        // Try to compare with arc2 (which is not the current value)
        let result = atomic.compare_exchange(Some(&arc2), Some(&arc3));
        assert!(result.is_err());

        // Value should remain unchanged
        let guard = atomic.load().unwrap();
        assert_eq!(*guard, 10);
    }

    #[test]
    fn test_compare_exchange_with_guard() {
        let arc1 = Arc::new(10);
        let arc2 = Arc::new(20);
        let atomic = AtomicArc::new(10);
        atomic.store(Some(&arc1));

        let guard = atomic.load().unwrap();
        let result = atomic.compare_exchange(Some(&guard), Some(&arc2));
        assert!(result.is_ok());

        let new_guard = atomic.load().unwrap();
        assert_eq!(*new_guard, 20);
    }

    #[test]
    fn test_from_arc() {
        let arc = Arc::new(42);
        let atomic = AtomicArc::new(0);
        atomic.store(Some(&arc));

        let guard = atomic.load().unwrap();
        assert_eq!(*guard, 42);
        assert_eq!(*arc, 42);
    }
}

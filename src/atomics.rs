use crate::statics::{acquire, release, retire};
use std::ptr::null;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Arc;

pub struct AtomicArc<T> {
    ptr: AtomicPtr<T>,
}

impl<T> AtomicArc<T> {
    /// Constructs a new `AtomicArc` given [`Some`] object or [`None`]. Analogous to [`Arc::new`].
    ///
    /// To directly convert an [`Arc`] to an `AtomicArc`, use `from` instead.
    ///
    /// # Examples
    /// ```
    /// use std::sync::atomic::Ordering::SeqCst;
    /// use aarc::atomics::AtomicArc;
    ///
    /// let atomic1: AtomicArc<Option<i32>> = AtomicArc::new(None);
    /// assert_eq!(atomic1.load(SeqCst), None);
    ///
    /// let atomic2 = AtomicArc::new(Some(42));
    /// assert_eq!(*atomic2.load(SeqCst).unwrap(), 42);
    /// ```
    pub fn new(item: Option<T>) -> Self {
        Self {
            ptr: item.map_or(AtomicPtr::default(), |inner| {
                AtomicPtr::new(Arc::into_raw(Arc::new(inner)).cast_mut())
            }),
        }
    }

    /// Loads the [`Arc`] with its reference count incremented appropriately if not [`None`].
    ///
    /// # Example
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::sync::atomic::Ordering::SeqCst;
    /// use aarc::atomics::AtomicArc;
    ///
    /// let atomic1 = AtomicArc::new(Some(42));
    /// let loaded = atomic1.load(SeqCst).unwrap();
    ///
    /// assert_eq!(Arc::strong_count(&loaded), 2);
    /// assert_eq!(*loaded, 42);
    /// ```
    pub fn load(&self, order: Ordering) -> Option<Arc<T>> {
        let ptr = self._load(order);
        if ptr.is_null() {
            None
        } else {
            unsafe { Some(Arc::from_raw(ptr)) }
        }
    }

    /// Stores `new` into `self` if `self` is equal to `current`.
    ///
    /// The comparison is shallow and is determined by pointer equality; see [`Arc::ptr_eq`].
    ///
    /// If the comparison succeeds, the return value will be an [`Ok`] containing the unit type
    /// (instead of a copy of `current`). This eliminates the overhead of providing the caller with
    /// a redundant [`Arc`].
    ///
    /// # Example:
    /// ```
    /// use std::sync::Arc;
    /// use std::sync::atomic::Ordering::SeqCst;
    /// use aarc::atomics::AtomicArc;
    ///
    /// let atomic1: AtomicArc<i32> = AtomicArc::new(None);
    /// let arc1 = Arc::new(42);
    ///
    /// assert!(atomic1.compare_exchange(None, Some(&arc1), SeqCst, SeqCst).is_ok());
    /// assert_eq!(*atomic1.load(SeqCst).unwrap(), *arc1);
    /// ```
    pub fn compare_exchange(
        &self,
        current: Option<&Arc<T>>,
        new: Option<&Arc<T>>,
        success: Ordering,
        failure: Ordering,
    ) -> Result<(), Option<Arc<T>>> {
        let c: *const T = current.map_or(null(), Arc::as_ptr);
        let n: *const T = new.map_or(null(), Arc::as_ptr);
        acquire();
        let result = match self
            .ptr
            .compare_exchange(c.cast_mut(), n.cast_mut(), success, failure)
        {
            Ok(before) => unsafe {
                if !before.is_null() {
                    retire(before);
                }
                if !n.is_null() {
                    Arc::increment_strong_count(n);
                }
                Ok(())
            },
            Err(before) => unsafe {
                if !before.is_null() {
                    Arc::increment_strong_count(before);
                    Err(Some(Arc::from_raw(before)))
                } else {
                    Err(None)
                }
            },
        };
        release();
        result
    }

    fn _load(&self, order: Ordering) -> *mut T {
        acquire();
        let ptr = self.ptr.load(order);
        if !ptr.is_null() {
            unsafe {
                Arc::increment_strong_count(ptr);
            }
        }
        release();
        ptr
    }
}

impl<T> Clone for AtomicArc<T> {
    fn clone(&self) -> Self {
        Self {
            ptr: AtomicPtr::new(self._load(SeqCst)),
        }
    }
}

impl<T> Default for AtomicArc<T> {
    /// Equivalent to `AtomicArc::new(None)`.
    fn default() -> Self {
        Self::new(None)
    }
}

impl<T> Drop for AtomicArc<T> {
    fn drop(&mut self) {
        let ptr = self.ptr.load(SeqCst);
        if !ptr.is_null() {
            retire(ptr);
        }
    }
}

impl<T> From<Arc<T>> for AtomicArc<T> {
    fn from(value: Arc<T>) -> Self {
        Self {
            ptr: AtomicPtr::new(Arc::into_raw(value).cast_mut()),
        }
    }
}

impl<T> From<Option<Arc<T>>> for AtomicArc<T> {
    fn from(value: Option<Arc<T>>) -> Self {
        Self {
            ptr: AtomicPtr::new(value.map_or(null(), Arc::into_raw).cast_mut()),
        }
    }
}

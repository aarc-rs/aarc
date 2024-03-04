use crate::statics::{begin_critical_section, end_critical_section, retire};
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::ptr::null;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Weak};

/// An atomically updatable variant of [`Arc`].
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
    /// use aarc::AtomicArc;
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

    /// Loads the [`Arc`] and increments its strong count appropriately if not [`None`].
    ///
    /// # Examples
    /// ```
    /// use std::sync::Arc;
    /// use std::sync::atomic::Ordering::SeqCst;
    /// use aarc::AtomicArc;
    ///
    /// let atomic = AtomicArc::new(None);
    /// atomic.store(Some(&Arc::new(42)), SeqCst);
    /// let loaded = atomic.load(SeqCst).unwrap();
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

    /// Stores `val` into `self`. See `AtomicArc::load` for examples.
    pub fn store(&self, val: Option<&Arc<T>>, order: Ordering) {
        let ptr: *const T = val.map_or(null(), Arc::as_ptr);
        unsafe {
            Arc::increment_strong_count(ptr);
        }
        let before = self.ptr.swap(ptr.cast_mut(), order);
        if !before.is_null() {
            retire::<_, true>(before);
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
    /// # Examples:
    /// ```
    /// use std::sync::Arc;
    /// use std::sync::atomic::Ordering::SeqCst;
    /// use aarc::AtomicArc;
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
        begin_critical_section();
        let result = match self
            .ptr
            .compare_exchange(c.cast_mut(), n.cast_mut(), success, failure)
        {
            Ok(before) => unsafe {
                if !before.is_null() {
                    retire::<_, true>(before);
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
        end_critical_section();
        result
    }

    fn _load(&self, order: Ordering) -> *mut T {
        begin_critical_section();
        let ptr = self.ptr.load(order);
        if !ptr.is_null() {
            unsafe {
                Arc::increment_strong_count(ptr);
            }
        }
        end_critical_section();
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
    /// Creates an empty `AtomicArc`. Equivalent to `AtomicArc::new(None)`.
    fn default() -> Self {
        Self {
            ptr: AtomicPtr::default(),
        }
    }
}

impl<T> Drop for AtomicArc<T> {
    fn drop(&mut self) {
        let ptr = self.ptr.load(SeqCst);
        if !ptr.is_null() {
            retire::<_, true>(ptr);
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

/// An atomically updatable variant of [`Weak`].
pub struct AtomicWeak<T> {
    ptr: AtomicPtr<T>,
}

impl<T> AtomicWeak<T> {
    /// Loads the [`Weak`] and increments its weak count appropriately if not [`None`].
    ///
    /// # Examples
    /// ```
    /// use std::sync::{Arc, Weak};
    /// use std::sync::atomic::Ordering::SeqCst;
    /// use aarc::{AtomicArc, AtomicWeak};
    ///
    /// let arc = Arc::new(42);
    /// let weak = Arc::downgrade(&arc);
    ///
    /// let atomic_weak = AtomicWeak::default();
    /// atomic_weak.store(Some(&weak), SeqCst);
    /// let loaded = atomic_weak.load(SeqCst).unwrap();
    ///
    /// assert_eq!(*loaded.upgrade().unwrap(), 42);
    /// assert_eq!(Weak::weak_count(&loaded), 3);
    /// ```
    pub fn load(&self, order: Ordering) -> Option<Weak<T>> {
        begin_critical_section();
        let ptr = self.ptr.load(order);
        let result = if !ptr.is_null() {
            unsafe { Some(clone_weak_from_raw(ptr)) }
        } else {
            None
        };
        end_critical_section();
        result
    }

    /// Stores `val` into `self`. See `AtomicWeak::load` for examples.
    pub fn store(&self, val: Option<&Weak<T>>, order: Ordering) {
        let ptr: *const T = val.map_or(null(), |w| {
            increment_weak_count(w);
            Weak::as_ptr(w)
        });
        let before = self.ptr.swap(ptr.cast_mut(), order);
        if !before.is_null() {
            retire::<_, false>(before);
        }
    }

    /// Stores `new` into `self` if `self` is equal to `current`.
    ///
    /// The comparison is shallow and is determined by pointer equality; see [`Weak::ptr_eq`].
    ///
    /// If the comparison succeeds, the return value will be an [`Ok`] containing the unit type
    /// (instead of a copy of `current`). This eliminates the overhead of providing the caller with
    /// a redundant [`Weak`].
    ///
    /// # Examples:
    /// ```
    /// use std::sync::Arc;
    /// use std::sync::atomic::Ordering::SeqCst;
    /// use aarc::AtomicWeak;
    ///
    /// let arc = Arc::new(42);
    /// let weak = Arc::downgrade(&arc);
    ///
    /// let atomic_weak = AtomicWeak::default();
    /// assert!(atomic_weak.compare_exchange(None, Some(&weak), SeqCst, SeqCst).is_ok());
    /// let loaded = atomic_weak.load(SeqCst).unwrap();
    /// assert_eq!(*loaded.upgrade().unwrap(), 42);
    /// ```
    pub fn compare_exchange(
        &self,
        current: Option<&Weak<T>>,
        new: Option<&Weak<T>>,
        success: Ordering,
        failure: Ordering,
    ) -> Result<(), Option<Weak<T>>> {
        let c: *const T = current.map_or(null(), Weak::as_ptr);
        let n: *const T = new.map_or(null(), Weak::as_ptr);
        begin_critical_section();
        let result = match self
            .ptr
            .compare_exchange(c.cast_mut(), n.cast_mut(), success, failure)
        {
            Ok(before) => {
                if !before.is_null() {
                    retire::<_, false>(before);
                }
                if let Some(weak_new) = new {
                    increment_weak_count(weak_new);
                }
                Ok(())
            }
            Err(before) => unsafe {
                if !before.is_null() {
                    Err(Some(clone_weak_from_raw(before)))
                } else {
                    Err(None)
                }
            },
        };
        end_critical_section();
        result
    }
}

impl<T> Clone for AtomicWeak<T> {
    fn clone(&self) -> Self {
        self.load(SeqCst).map_or(Self::default(), |weak| Self {
            ptr: AtomicPtr::new(Weak::into_raw(weak).cast_mut()),
        })
    }
}

impl<T> Default for AtomicWeak<T> {
    /// Creates an empty `AtomicWeak`.
    fn default() -> Self {
        Self {
            ptr: AtomicPtr::default(),
        }
    }
}

impl<T> Drop for AtomicWeak<T> {
    fn drop(&mut self) {
        let ptr = self.ptr.load(SeqCst);
        if !ptr.is_null() {
            retire::<_, false>(ptr);
        }
    }
}

impl<T> From<Weak<T>> for AtomicWeak<T> {
    fn from(value: Weak<T>) -> Self {
        Self {
            ptr: AtomicPtr::new(Weak::into_raw(value).cast_mut()),
        }
    }
}

impl<T> From<Option<Weak<T>>> for AtomicWeak<T> {
    fn from(value: Option<Weak<T>>) -> Self {
        Self {
            ptr: AtomicPtr::new(value.map_or(null(), Weak::into_raw).cast_mut()),
        }
    }
}

fn increment_weak_count<T>(w: &Weak<T>) {
    _ = ManuallyDrop::new(w.clone());
}

unsafe fn clone_weak_from_raw<T>(ptr: *const T) -> Weak<T> {
    ManuallyDrop::new(Weak::from_raw(ptr)).deref().clone()
}

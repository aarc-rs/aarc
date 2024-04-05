use crate::smr::drc::{Protect, ProtectPtr, Retire};
use crate::smr::standard_reclaimer::StandardReclaimer;
use crate::Snapshot;
use std::any::TypeId;
use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::ptr;
use std::ptr::{null, null_mut};
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed};
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// An [`Arc`] with an atomically updatable pointer.
///
/// Usage notes:
/// * An `AtomicArc` can store [`None`]. A hypothetical `Option<AtomicArc<T>>` would be pointless,
/// as the variable would lose its atomicity, so the
/// [null pointer optimization](https://doc.rust-lang.org/std/option/index.html#representation) is
/// utilized to intrinsically support [`Option`].
/// * An `AtomicArc` contributes to the strong count of the pointed-to allocation, if any. However,
/// it does not implement [`Deref`], so a method like `load` must be used to obtain an [`Arc`] or a
/// [`Snapshot`] through which the data can be read.
/// * `T` must be [`Sized`] for compatibility with [`AtomicPtr`].
/// * All methods use load-acquire, store-release semantics.
/// * When an `AtomicArc` is dropped or updated, the strong count may not be immediately
/// decremented. Thus:
///     * `T` must be `'static` to support delayed deallocations.
///     * [`Arc::strong_count`] will likely be an overestimate.

///
/// # Examples
/// ```
/// use std::sync::Arc;
/// use aarc::{AtomicArc, Snapshot};
///
/// let atomic = AtomicArc::new(Some(53)); // +1 strong count on val 53
///
/// let snapshot53 = atomic.load::<Snapshot<_>>().unwrap(); // snapshot doesn't affect counts
/// assert_eq!(*snapshot53, 53);
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
        let mut to_retire: *mut T = null_mut();
        let guard = R::protect();
        let result = match self
            .ptr
            .compare_exchange(c.cast_mut(), n.cast_mut(), AcqRel, Acquire)
        {
            Ok(before) => unsafe {
                if !n.is_null() && !ptr::eq(n, before) {
                    Arc::increment_strong_count(n);
                    to_retire = before;
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
        };
        drop(guard); // drop it early because retire could take a (relatively) long time.
        if !to_retire.is_null() {
            R::retire(to_retire as *mut u8, get_drop_fn::<T, true>());
        }
        result
    }

    /// Loads and returns the desired smart pointer type (or [`None`] if it is null).
    pub fn load<V: SmartPtr<T>>(&self) -> Option<V> {
        let _guard = R::protect();
        let ptr = self.ptr.load(Acquire);
        if ptr.is_null() {
            None
        } else {
            unsafe { Some(V::clone_from_raw(ptr)) }
        }
    }

    /// Stores `new`'s pointer (or [`None`]) into `self`.
    pub fn store<N: StrongPtr<T>>(&self, new: Option<&N>) {
        // TODO: rework this method to possibly take ownership of new (avoid increment).
        let ptr: *const T = new.map_or(null(), N::as_ptr);
        if !ptr.is_null() {
            unsafe {
                Arc::increment_strong_count(ptr);
            }
        }
        let before = self.ptr.swap(ptr.cast_mut(), AcqRel);
        if !before.is_null() {
            R::retire(before as *mut u8, get_drop_fn::<T, true>());
        }
    }
}

impl<T: 'static, R: Protect + Retire> Clone for AtomicArc<T, R> {
    fn clone(&self) -> Self {
        let _guard = R::protect();
        let ptr = self.ptr.load(Acquire);
        if !ptr.is_null() {
            unsafe {
                Arc::increment_strong_count(ptr);
            }
        }
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
            R::retire(ptr as *mut u8, get_drop_fn::<T, true>());
        }
    }
}

unsafe impl<T: 'static + Send + Sync, R: Protect + Retire> Send for AtomicArc<T, R> {}

unsafe impl<T: 'static + Send + Sync, R: Protect + Retire> Sync for AtomicArc<T, R> {}

/// A [`Weak`] with an atomically updatable pointer.
///
/// See [`AtomicArc`] for usage notes. `AtomicWeak` differs only in that it contributes to the weak
/// count instead of the strong count.
///
/// # Examples
/// ```
/// use std::sync::Arc;
/// use aarc::{AtomicWeak, Snapshot};
///
/// let arc1 = Arc::new(53); // +1 strong count
///
/// let atomic = AtomicWeak::<_>::from(&arc1); // +1 weak count
///
/// let weak = atomic.load().unwrap(); // +1 weak count
/// assert_eq!(Arc::strong_count(&arc1), 1);
/// assert_eq!(Arc::weak_count(&arc1), 2);
/// ```
pub struct AtomicWeak<T: 'static, R: Protect + Retire = StandardReclaimer> {
    ptr: AtomicPtr<T>,
    phantom_r: PhantomData<R>,
}

impl<T: 'static, R: Protect + Retire> AtomicWeak<T, R> {
    /// See [`AtomicArc::compare_exchange`]. This method behaves similarly, except that the return
    /// type for the failure case cannot be specified by the caller; it will be a [`Weak`].
    pub fn compare_exchange<C, N>(
        &self,
        current: Option<&C>,
        new: Option<&N>,
    ) -> Result<(), Option<Weak<T>>>
    where
        C: SmartPtr<T>,
        N: SmartPtr<T>,
    {
        let c: *const T = current.map_or(null(), C::as_ptr);
        let n: *const T = new.map_or(null(), N::as_ptr);
        let mut to_retire: *mut T = null_mut();
        let guard = R::protect();
        let result = match self
            .ptr
            .compare_exchange(c.cast_mut(), n.cast_mut(), AcqRel, Acquire)
        {
            Ok(before) => unsafe {
                if !n.is_null() && !ptr::eq(n, before) {
                    _ = ManuallyDrop::new(Weak::from_raw(before)).clone();
                    to_retire = before;
                }
                Ok(())
            },
            Err(before) => unsafe {
                if before.is_null() {
                    Err(None)
                } else {
                    Err(Some(Weak::clone_from_raw(before)))
                }
            },
        };
        drop(guard); // drop it early because retire could take a (relatively) long time.
        if !to_retire.is_null() {
            R::retire(to_retire as *mut u8, get_drop_fn::<T, false>());
        }
        result
    }

    /// Loads the pointer and returns a [`Weak`] (or [`None`] if it is null).
    pub fn load(&self) -> Option<Weak<T>> {
        let _guard = R::protect();
        let ptr = self.ptr.load(Acquire);
        if ptr.is_null() {
            None
        } else {
            unsafe { Some(Weak::clone_from_raw(ptr)) }
        }
    }

    /// Stores `new`'s pointer (or [`None`]) into `self`.
    pub fn store<N: SmartPtr<T>>(&self, new: Option<&N>) {
        let ptr: *const T = new.map_or(null(), N::as_ptr);
        if !ptr.is_null() {
            unsafe {
                _ = ManuallyDrop::new(Weak::from_raw(ptr)).clone();
            }
        }
        let before = self.ptr.swap(ptr.cast_mut(), AcqRel);
        if !before.is_null() {
            R::retire(before as *mut u8, get_drop_fn::<T, false>());
        }
    }

    /// Loads an [`Arc`] if the strong count is at least one. Analogous to [`Weak::upgrade`].
    pub fn upgrade(&self) -> Option<Arc<T>> {
        let _guard = R::protect();
        let ptr = self.ptr.load(Acquire);
        if ptr.is_null() {
            None
        } else {
            unsafe {
                ManuallyDrop::new(Weak::from_raw(ptr)).upgrade()
            }
        }
    }
}

impl<T: 'static, R: Protect + Retire> Clone for AtomicWeak<T, R> {
    fn clone(&self) -> Self {
        let _guard = R::protect();
        let ptr = self.ptr.load(Acquire);
        if !ptr.is_null() {
            unsafe {
                _ = ManuallyDrop::new(Weak::from_raw(ptr)).clone();
            }
        }
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
            R::retire(ptr as *mut u8, get_drop_fn::<T, false>());
        }
    }
}

impl<T: 'static, R: Protect + ProtectPtr + Retire> From<&Snapshot<T, R>> for AtomicArc<T, R> {
    fn from(value: &Snapshot<T, R>) -> Self {
        unsafe {
            let ptr = Snapshot::as_ptr(value);
            Arc::increment_strong_count(ptr);
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
            _ = ManuallyDrop::new(Weak::from_raw(ptr)).clone();
            Self {
                ptr: AtomicPtr::new(ptr.cast_mut()),
                phantom_r: PhantomData,
            }
        }
    }
}

impl<T: 'static, R: Protect + Retire> From<&Arc<T>> for AtomicArc<T, R> {
    fn from(value: &Arc<T>) -> Self {
        unsafe {
            let ptr = Arc::as_ptr(value);
            Arc::increment_strong_count(ptr);
            Self {
                ptr: AtomicPtr::new(ptr.cast_mut()),
                phantom: PhantomData,
                phantom_r: PhantomData,
            }
        }
    }
}

impl<T: 'static, R: Protect + Retire> From<&Arc<T>> for AtomicWeak<T, R> {
    fn from(value: &Arc<T>) -> Self {
        unsafe {
            let ptr = Arc::as_ptr(value);
            _ = ManuallyDrop::new(Weak::from_raw(ptr)).clone();
            Self {
                ptr: AtomicPtr::new(ptr.cast_mut()),
                phantom_r: PhantomData,
            }
        }
    }
}

unsafe impl<T: 'static + Send + Sync, R: Protect + Retire> Send for AtomicWeak<T, R> {}

unsafe impl<T: 'static + Send + Sync, R: Protect + Retire> Sync for AtomicWeak<T, R> {}

type FnLookup = HashMap<(TypeId, bool), fn(*mut u8)>;

// A thread will only lock the mutex once per key to populate the thread-local cache.
static DROP_FN_LOOKUP: OnceLock<Mutex<FnLookup>> = OnceLock::new();

thread_local! {
    static LOCAL_DROP_FN_LOOKUP: RefCell<FnLookup> = RefCell::default();
}

fn get_drop_fn<T: 'static, const IS_ARC: bool>() -> fn(*mut u8) {
    LOCAL_DROP_FN_LOOKUP.with_borrow_mut(|lookup| {
        let key = (TypeId::of::<T>(), IS_ARC);
        *lookup.entry(key).or_insert_with(|| {
            let mut m = DROP_FN_LOOKUP.get_or_init(Mutex::default).lock().unwrap();
            *m.entry(key).or_insert_with(|| {
                if IS_ARC {
                    |ptr: *mut u8| unsafe { drop(Arc::from_raw(ptr as *const T)) }
                } else {
                    |ptr: *mut u8| unsafe { drop(Weak::from_raw(ptr as *const T)) }
                }
            })
        })
    })
}

/// A trait to generalize the [`Arc::as_ptr`] method.
pub trait AsPtr<T> {
    fn as_ptr(this: &Self) -> *const T;
}

impl<T> AsPtr<T> for Arc<T> {
    fn as_ptr(this: &Self) -> *const T {
        Arc::as_ptr(this)
    }
}

impl<T> AsPtr<T> for Weak<T> {
    fn as_ptr(this: &Self) -> *const T {
        this.as_ptr()
    }
}

pub trait CloneFromRaw<T> {
    #[allow(clippy::missing_safety_doc)]
    unsafe fn clone_from_raw(ptr: *const T) -> Self;
}

impl<T> CloneFromRaw<T> for Arc<T> {
    unsafe fn clone_from_raw(ptr: *const T) -> Self {
        (*ManuallyDrop::new(Self::from_raw(ptr))).clone()
    }
}

impl<T> CloneFromRaw<T> for Weak<T> {
    unsafe fn clone_from_raw(ptr: *const T) -> Self {
        (*ManuallyDrop::new(Self::from_raw(ptr))).clone()
    }
}

/// A marker trait for smart pointers that prevent deallocation of the object: [`Arc`] and
/// [`Snapshot`], but not [`Weak`].
pub trait StrongPtr<T>: Deref + SmartPtr<T> {}

impl<T, X> StrongPtr<T> for X where X: Deref + SmartPtr<T> {}

/// A marker trait for all smart pointers: [`Arc`], [`Weak`], and [`Snapshot`].
pub trait SmartPtr<T>: AsPtr<T> + CloneFromRaw<T> {}

impl<T, X> SmartPtr<T> for X where X: AsPtr<T> + CloneFromRaw<T> {}

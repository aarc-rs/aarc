use crate::smr::drc::{ProtectPtr, Release, Retire};
use crate::smr::standard_reclaimer::StandardReclaimer;
use crate::utils::helpers::alloc_box_ptr;
use crate::utils::sticky_counter::StickyCounter;
use std::alloc::{dealloc, Layout};
use std::any::TypeId;
use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::ops::Deref;
use std::ptr::NonNull;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::{Relaxed, SeqCst};
use std::sync::{Mutex, OnceLock};
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
/// All methods on this struct will behave identically to their [`std::sync::Arc`] counterparts,
/// unless otherwise noted.
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
        assert!(Self::try_increment_strong_count(ptr));
    }
    pub(crate) unsafe fn try_increment_strong_count(ptr: *const T) -> bool {
        (*(ptr as *const ArcInner<T>))
            .strong
            .try_increment()
            .is_ok()
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
                    strong: StickyCounter::default(),
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
    /// Note: When an `Arc` is dropped, the strong count may not be immediately decremented, so
    /// this will likely be an overestimate.
    pub fn strong_count(this: &Self) -> usize {
        unsafe { (*this.ptr.as_ptr()).strong.load() }
    }
    /// Note: When a `Weak` is dropped, the weak count may not be immediately decremented, so this
    /// will likely be an overestimate.
    pub fn weak_count(this: &Self) -> usize {
        unsafe { (*this.ptr.as_ptr()).weak.load(Relaxed) - 1 }
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
        R::retire(self.ptr.as_ptr() as *mut u8, get_drop_fn::<T, R, true>());
    }
}

unsafe impl<T: 'static + Send + Sync, R: Retire> Send for Arc<T, R> {}

unsafe impl<T: 'static + Send + Sync, R: Retire> Sync for Arc<T, R> {}

/// A reimplementation of [`std::sync::Weak`].
///
/// See [`Arc`] for details on how this struct differs from the standard library's.
///
/// All methods on this struct will behave identically to their [`std::sync::Weak`] counterparts,
/// unless otherwise noted.
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
        (*(ptr as *const ArcInner<T>)).weak.fetch_add(1, SeqCst);
    }
    pub fn into_raw(self) -> *const T {
        let ptr = Self::as_ptr(&self);
        mem::forget(self);
        ptr
    }
    /// Note: This method is generic; the caller may specify `V` to obtain either an `Arc` or a
    /// `Snapshot`.
    pub fn upgrade<V: StrongPtr<T>>(&self) -> Option<V> {
        unsafe { V::try_clone_from_raw(Self::as_ptr(self)) }
    }
}

impl<T: 'static, R: Retire> Drop for Weak<T, R> {
    fn drop(&mut self) {
        R::retire(self.ptr.as_ptr() as *mut u8, get_drop_fn::<T, R, false>());
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
struct ArcInner<T> {
    data: T,
    strong: StickyCounter,
    weak: AtomicUsize,
}

type FnLookup = HashMap<(TypeId, bool), fn(*mut u8)>;

// The mutex will only be used during initialization, in std environments, when a thread "sees" a
// type T for the first time.
static DROP_FN_LOOKUP: OnceLock<Mutex<FnLookup>> = OnceLock::new();

thread_local! {
    static LOCAL_DROP_FN_LOOKUP: RefCell<FnLookup> = RefCell::default();
}

fn get_drop_fn<T: 'static, R: Retire, const IS_ARC: bool>() -> fn(*mut u8) {
    LOCAL_DROP_FN_LOOKUP.with_borrow_mut(|lookup| {
        let key = (TypeId::of::<T>(), IS_ARC);
        *lookup.entry(key).or_insert_with(|| {
            let mut m = DROP_FN_LOOKUP.get_or_init(Mutex::default).lock().unwrap();
            *m.entry(key).or_insert_with(|| {
                if IS_ARC {
                    |ptr: *mut u8| unsafe {
                        if (*(ptr as *mut ArcInner<T>)).strong.decrement() == 1 {
                            // fence(Acquire);
                            ptr::drop_in_place(ptr as *mut T);
                            drop(Weak::<T, R>::from_raw(ptr as *const T));
                        }
                    }
                } else {
                    |ptr: *mut u8| unsafe {
                        if (*(ptr as *mut ArcInner<T>)).weak.fetch_sub(1, SeqCst) == 1 {
                            // fence(Acquire);
                            dealloc(ptr, Layout::new::<ArcInner<T>>())
                        }
                    }
                }
            })
        })
    })
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
    #[allow(clippy::missing_safety_doc)]
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
    #[allow(clippy::missing_safety_doc)]
    unsafe fn try_clone_from_raw(ptr: *const T) -> Option<Self>;
}

impl<T: 'static, R: Retire> TryCloneFromRaw<T> for Arc<T, R> {
    unsafe fn try_clone_from_raw(ptr: *const T) -> Option<Self> {
        Self::try_increment_strong_count(ptr).then(|| Self::from_raw(ptr))
    }
}

impl<T: 'static, R: Retire> TryCloneFromRaw<T> for Weak<T, R> {
    unsafe fn try_clone_from_raw(ptr: *const T) -> Option<Self> {
        Self::increment_weak_count(ptr);
        Some(Self::from_raw(ptr))
    }
}

impl<T: 'static, R: ProtectPtr> TryCloneFromRaw<T> for Snapshot<T, R> {
    unsafe fn try_clone_from_raw(ptr: *const T) -> Option<Self> {
        let inner = ptr as *mut ArcInner<T>;
        let handle = R::protect_ptr(ptr as *mut u8);
        if (*inner).strong.load() == 0 {
            handle.release();
            None
        } else {
            Some(Self {
                ptr: NonNull::new_unchecked(inner),
                phantom: PhantomData,
                handle,
            })
        }
    }
}

/// A marker trait for smart pointers that prevent deallocation of the object: [`Arc`] and
/// [`Snapshot`], but not [`Weak`].
pub trait StrongPtr<T>: Deref + SmartPtr<T> {}

impl<T, X> StrongPtr<T> for X where X: Deref + SmartPtr<T> {}

/// A marker trait representing all smart pointers in this crate: [`Arc`], [`Weak`], and
/// [`Snapshot`].
pub trait SmartPtr<T>: AsPtr<T> + CloneFromRaw<T> + TryCloneFromRaw<T> {}

impl<T, X> SmartPtr<T> for X where X: AsPtr<T> + CloneFromRaw<T> + TryCloneFromRaw<T> {}

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
    use crate::smart_ptrs::{Arc, Weak};
    use crate::smr::standard_reclaimer::StandardReclaimer;
    use std::cell::RefCell;
    use std::ptr::addr_of_mut;

    #[test]
    fn test_arc_cascading_drop() {
        const NODES: usize = 5;

        struct Node {
            val: usize,
            _next: Option<Arc<Self>>,
            push_on_drop: *mut Vec<usize>,
        }
        impl Drop for Node {
            fn drop(&mut self) {
                unsafe {
                    (*self.push_on_drop).push(self.val);
                }
            }
        }

        let mut dropped_vals = Vec::new();
        let mut head: Option<Arc<Node>> = None;
        for i in 0..NODES {
            head = Some(Arc::new(Node {
                val: i,
                _next: head.as_ref().map(Arc::clone),
                push_on_drop: addr_of_mut!(dropped_vals),
            }));
        }

        drop(head);
        for i in 0..NODES {
            assert_eq!(dropped_vals.len(), i);
            StandardReclaimer::cleanup_owned_slot();
            assert_eq!(dropped_vals.len(), i + 1);
            assert_eq!(dropped_vals[i], NODES - i - 1);
        }
    }

    #[test]
    fn test_arc_weak_cycle() {
        struct Node {
            val: usize,
            _prev: Option<Weak<RefCell<Self>>>,
            _next: Option<Arc<RefCell<Self>>>,
            push_on_drop: *mut Vec<usize>,
        }
        impl Drop for Node {
            fn drop(&mut self) {
                unsafe {
                    (*self.push_on_drop).push(self.val);
                }
            }
        }

        let mut dropped_vals = Vec::new();
        let n0 = Arc::new(RefCell::new(Node {
            val: 0,
            _prev: None,
            _next: None,
            push_on_drop: addr_of_mut!(dropped_vals),
        }));
        let n1 = Arc::new(RefCell::new(Node {
            val: 1,
            _prev: Some(Arc::downgrade(&n0)),
            _next: None,
            push_on_drop: addr_of_mut!(dropped_vals),
        }));
        n0.borrow_mut()._next = Some(n1.clone());

        drop(n1);
        drop(n0);
        assert_eq!(dropped_vals.len(), 0);
        StandardReclaimer::cleanup_owned_slot();
        assert_eq!(dropped_vals.len(), 1);
        assert_eq!(dropped_vals[0], 0);
        StandardReclaimer::cleanup_owned_slot();
        assert_eq!(dropped_vals.len(), 2);
        assert_eq!(dropped_vals[1], 1);
    }
}

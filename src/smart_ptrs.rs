use std::alloc::{dealloc, Layout};
use std::marker::PhantomData;
use std::mem::{forget, ManuallyDrop};
use std::ops::Deref;
use std::ptr::{addr_of_mut, drop_in_place, NonNull};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::{Relaxed, SeqCst};

use fast_smr::smr;
use fast_smr::smr::{load_era, retire};

/// An [`Arc`]-like smart pointer that can be loaded from atomics.
///
/// Usage notes:
/// * A `Guard` should be used as a temporary variable within a local scope, not as a replacement
///   for [`Arc`] in a data structure.
/// * `Guard` implements `Deref` and prevents deallocation like [`Arc`], but it does not contribute
///   to the strong count.
pub struct Guard<T: 'static> {
    pub(crate) guard: smr::Guard<T>,
}

impl<T: 'static> Deref for Guard<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.guard.as_ptr() }
    }
}

impl<T: 'static> From<&Guard<T>> for Arc<T> {
    fn from(value: &Guard<T>) -> Self {
        unsafe {
            let ptr = value.guard.as_ptr();
            Self::increment_strong_count(ptr);
            Self {
                ptr: NonNull::new_unchecked(find_inner_ptr(ptr).cast_mut()),
                phantom: PhantomData,
            }
        }
    }
}

impl<T: 'static> From<&Guard<T>> for Weak<T> {
    fn from(value: &Guard<T>) -> Self {
        unsafe {
            let ptr = value.guard.as_ptr();
            Self::increment_weak_count(ptr);
            Self {
                ptr: NonNull::new_unchecked(find_inner_ptr(ptr).cast_mut()),
            }
        }
    }
}

/// A drop-in replacement for [`std::sync::Arc`].
pub struct Arc<T: 'static> {
    ptr: NonNull<ArcInner<T>>,
    phantom: PhantomData<ArcInner<T>>,
}

impl<T: 'static> Arc<T> {
    pub fn new(data: T) -> Self {
        unsafe {
            let ptr = NonNull::new_unchecked(Box::into_raw(Box::new(ArcInner {
                strong_count: AtomicUsize::new(1),
                weak_count: AtomicUsize::new(1),
                birth_era: load_era(),
                data,
            })));
            Self {
                ptr,
                phantom: PhantomData,
            }
        }
    }
    pub fn into_raw(this: Self) -> *const T {
        let ptr = this.as_ptr();
        forget(this);
        ptr
    }
    /// # Safety
    /// See [`std::sync::Arc::from_raw`].
    pub unsafe fn from_raw(ptr: *const T) -> Self {
        Self {
            ptr: NonNull::new_unchecked(find_inner_ptr(ptr).cast_mut()),
            phantom: PhantomData,
        }
    }
    pub(crate) unsafe fn strong_count_raw(ptr: *const T) -> usize {
        (*find_inner_ptr(ptr)).strong_count.load(SeqCst)
    }
    pub(crate) unsafe fn increment_strong_count(ptr: *const T) {
        _ = ManuallyDrop::new(Self::from_raw(ptr)).clone();
    }
}

impl<T: 'static> Clone for Arc<T> {
    fn clone(&self) -> Self {
        unsafe {
            self.ptr.as_ref().strong_count.fetch_add(1, SeqCst);
        }
        Self {
            ptr: self.ptr,
            phantom: PhantomData,
        }
    }
}

impl<T: 'static> Deref for Arc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &self.ptr.as_ref().data }
    }
}

impl<T: 'static> Drop for Arc<T> {
    fn drop(&mut self) {
        let birth_era = unsafe { self.ptr.as_ref().birth_era };
        retire(self.ptr.cast(), decrement_strong_count::<T>, birth_era);
    }
}

fn decrement_strong_count<T>(ptr: NonNull<u8>) {
    unsafe {
        let inner = ptr.cast::<ArcInner<T>>().as_ptr();
        if (*inner).strong_count.fetch_sub(1, SeqCst) == 1 {
            drop_in_place(&mut (*inner).data);
            decrement_weak_count::<T>(ptr);
        }
    }
}

/// A drop-in replacement for [`std::sync::Weak`].
pub struct Weak<T: 'static> {
    ptr: NonNull<ArcInner<T>>,
}

impl<T: 'static> Weak<T> {
    pub(crate) unsafe fn increment_weak_count(ptr: *const T) {
        _ = ManuallyDrop::new(Self::from_raw(ptr)).clone();
    }
    pub(crate) unsafe fn from_raw(ptr: *const T) -> Self {
        Self {
            ptr: NonNull::new_unchecked(find_inner_ptr(ptr).cast_mut()),
        }
    }
}

impl<T: 'static> Clone for Weak<T> {
    fn clone(&self) -> Self {
        unsafe {
            self.ptr.as_ref().weak_count.fetch_add(1, SeqCst);
        }
        Self { ptr: self.ptr }
    }
}

impl<T: 'static> Drop for Weak<T> {
    fn drop(&mut self) {
        let birth_era = unsafe { self.ptr.as_ref().birth_era };
        retire(self.ptr.cast(), decrement_weak_count::<T>, birth_era);
    }
}

fn decrement_weak_count<T>(ptr: NonNull<u8>) {
    unsafe {
        let inner = ptr.cast::<ArcInner<T>>().as_ptr();
        if (*inner).weak_count.fetch_sub(1, SeqCst) == 1 {
            dealloc(ptr.as_ptr(), Layout::new::<ArcInner<T>>());
        }
    }
}

unsafe fn find_inner_ptr<T>(ptr: *const T) -> *const ArcInner<T> {
    let layout = Layout::new::<ArcInner<()>>();
    let offset = layout.size() + padding_needed_for(&layout, align_of::<T>());
    ptr.byte_sub(offset) as *const ArcInner<T>
}

// See: [`Layout::padding_needed_for`]
fn padding_needed_for(layout: &Layout, align: usize) -> usize {
    let len = layout.size();
    let len_rounded_up = len.wrapping_add(align).wrapping_sub(1) & !align.wrapping_sub(1);
    len_rounded_up.wrapping_sub(len)
}

#[repr(C)]
struct ArcInner<T> {
    strong_count: AtomicUsize,
    weak_count: AtomicUsize,
    birth_era: u64,
    data: T,
}

/// A trait for extracting a raw pointer from a smart pointer.
pub trait AsPtr {
    type Target;

    fn as_ptr(&self) -> *const Self::Target;
}

impl<T: 'static> AsPtr for Arc<T> {
    type Target = T;

    fn as_ptr(&self) -> *const T {
        unsafe { addr_of_mut!((*self.ptr.as_ptr()).data) }
    }
}

impl<T: 'static> AsPtr for Weak<T> {
    type Target = T;

    fn as_ptr(&self) -> *const T {
        unsafe { addr_of_mut!((*self.ptr.as_ptr()).data) }
    }
}

impl<T: 'static> AsPtr for Guard<T> {
    type Target = T;

    fn as_ptr(&self) -> *const T {
        self.guard.as_ptr().cast_const()
    }
}

/// A marker trait for types that prevent deallocation ([`Arc`] and [`Guard`]).
pub trait StrongPtr {}
impl<T: 'static> StrongPtr for Arc<T> {}
impl<T: 'static> StrongPtr for Guard<T> {}

pub trait RefCount {
    fn strong_count(&self) -> usize;
    fn weak_count(&self) -> usize;
}

impl<T: AsPtr> RefCount for T {
    fn strong_count(&self) -> usize {
        unsafe {
            let inner = find_inner_ptr(self.as_ptr());
            (*inner).strong_count.load(Relaxed)
        }
    }

    fn weak_count(&self) -> usize {
        unsafe {
            let inner = find_inner_ptr(self.as_ptr());
            (*inner).weak_count.load(Relaxed) - 1
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::smart_ptrs::Arc;

    #[test]
    fn test_arc() {
        let x = Arc::new(55usize);
        assert_eq!(*x, 55);
        unsafe {
            let y = Arc::from_raw(Arc::into_raw(x));
            assert_eq!(*y, 55);
        }
    }
}

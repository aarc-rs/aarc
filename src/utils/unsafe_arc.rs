use crate::utils::helpers::{alloc_box_ptr, dealloc_box_ptr};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::sync::atomic::{fence, AtomicUsize};

/// A slightly more efficient and convenient Arc for internal use only.
/// It has no weak count, implements `DerefMut`, and can be initialized with an arbitrary ref count.
pub(crate) struct UnsafeArc<T> {
    ptr: NonNull<UnsafeArcInner<T>>,
    phantom: PhantomData<UnsafeArcInner<T>>,
}

impl<T> UnsafeArc<T> {
    pub(crate) fn as_ptr(this: &Self) -> *mut T {
        this.ptr.as_ptr().cast::<T>()
    }
    pub(crate) unsafe fn decrement_ref_count(ptr: *mut T) {
        unsafe {
            let inner = ptr.cast::<UnsafeArcInner<T>>();
            if (*inner).ref_count.fetch_sub(1, Release) == 1 {
                fence(Acquire);
                dealloc_box_ptr(inner);
            }
        }
    }
    pub(crate) unsafe fn from_raw(ptr: *mut T) -> Self {
        Self {
            ptr: NonNull::new_unchecked(ptr.cast::<UnsafeArcInner<T>>()),
            phantom: PhantomData,
        }
    }
    pub(crate) fn new(item: T, starting_ref_count: usize) -> Self {
        unsafe {
            Self {
                ptr: NonNull::new_unchecked(alloc_box_ptr(UnsafeArcInner {
                    item,
                    ref_count: AtomicUsize::new(starting_ref_count),
                })),
                phantom: PhantomData,
            }
        }
    }
}

impl<T> Clone for UnsafeArc<T> {
    fn clone(&self) -> Self {
        unsafe {
            (*self.ptr.as_ptr()).ref_count.fetch_add(1, Relaxed);
        }
        Self {
            ptr: self.ptr,
            phantom: PhantomData,
        }
    }
}

impl<T: Default> Default for UnsafeArc<T> {
    fn default() -> Self {
        Self::new(T::default(), 1)
    }
}

impl<T> Deref for UnsafeArc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &(*self.ptr.as_ptr()).item }
    }
}

impl<T> DerefMut for UnsafeArc<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut (*self.ptr.as_ptr()).item }
    }
}

impl<T> Drop for UnsafeArc<T> {
    fn drop(&mut self) {
        unsafe {
            Self::decrement_ref_count(Self::as_ptr(self));
        }
    }
}

struct UnsafeArcInner<T> {
    item: T,
    ref_count: AtomicUsize,
}

#[cfg(test)]
mod tests {
    use crate::utils::unsafe_arc::UnsafeArc;
    use std::sync::atomic::Ordering::SeqCst;

    #[test]
    fn test_no_leak() {
        let a = UnsafeArc::new("hello".to_string(), 2);
        unsafe {
            UnsafeArc::decrement_ref_count(UnsafeArc::as_ptr(&a));
            assert_eq!((*a.ptr.as_ptr()).ref_count.load(SeqCst), 1);
        }
    }
}

use crate::utils::helpers::{alloc_box_ptr, dealloc_box_ptr};
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::sync::atomic::{fence, AtomicUsize};

/// A slightly more efficient and convenient Arc for internal use.
pub(crate) struct UnsafeArc<T> {
    pub(crate) ptr: *mut UnsafeArcInner<T>,
}

impl<T> UnsafeArc<T> {
    pub(crate) fn new(item: T, starting_ref_count: usize) -> Self {
        Self {
            ptr: alloc_box_ptr(UnsafeArcInner {
                item,
                ref_count: AtomicUsize::new(starting_ref_count),
            }),
        }
    }
    pub(crate) fn increment_ref_count(&self) {
        unsafe {
            (*self.ptr).ref_count.fetch_add(1, Relaxed);
        }
    }
    pub(crate) fn decrement_ref_count(&self) {
        unsafe {
            if (*self.ptr).ref_count.fetch_sub(1, Release) == 1 {
                fence(Acquire);
                dealloc_box_ptr(self.ptr);
            }
        }
    }
    pub(crate) unsafe fn from_raw(ptr: *mut UnsafeArcInner<T>) -> Self {
        Self { ptr }
    }
}

impl<T> Clone for UnsafeArc<T> {
    fn clone(&self) -> Self {
        self.increment_ref_count();
        Self { ptr: self.ptr }
    }
}

impl<T: Default> Default for UnsafeArc<T> {
    fn default() -> Self {
        Self {
            ptr: alloc_box_ptr(UnsafeArcInner {
                item: T::default(),
                ref_count: AtomicUsize::new(1),
            }),
        }
    }
}

impl<T> Drop for UnsafeArc<T> {
    fn drop(&mut self) {
        self.decrement_ref_count();
    }
}

pub(crate) struct UnsafeArcInner<T> {
    pub(crate) item: T,
    pub(crate) ref_count: AtomicUsize,
}

#[cfg(test)]
mod tests {
    use crate::utils::unsafe_arc::UnsafeArc;
    use std::sync::atomic::Ordering::SeqCst;

    #[test]
    fn test_no_leak() {
        let a = UnsafeArc::new("hello".to_string(), 2);
        UnsafeArc::increment_ref_count(&a);
        UnsafeArc::decrement_ref_count(&a);
        UnsafeArc::decrement_ref_count(&a);
        unsafe {
            assert_eq!((*a.ptr).ref_count.load(SeqCst), 1);
        }
    }
}

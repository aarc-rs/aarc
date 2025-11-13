use std::alloc::Layout;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::ops::Deref;
use std::ptr::{addr_of, NonNull};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::thread::available_parallelism;

use fast_smr::smr;
use fast_smr::smr::{Reclaimer, ThreadContext};

// The global default `Reclaimer`.
pub(crate) static RECLAIMER: Reclaimer = Reclaimer::new();

thread_local! {
    pub(crate) static CTX: RefCell<ThreadContext<'static>> = RefCell::new(
        RECLAIMER.get_ctx(available_parallelism().map_or(8usize, NonZeroUsize::get)));
}

/// An [`Arc`]-like smart pointer that can be loaded from `AtomicArc`.
///
/// Usage notes:
/// * A `Guard` should be used as a temporary variable within a local scope, not as a replacement
///   for [`Arc`] in a data structure.
/// * `Guard` implements `Deref` and prevents deallocation like [`Arc`], but it does not contribute
///   to the ref count.
pub struct Guard<'a, T> {
    pub(crate) guard: smr::Guard<'a, ArcInner<T>>,
}

impl<'a, T> Guard<'a, T> {
    pub(crate) fn inner_ptr(this: &Self) -> *const ArcInner<T> {
        this.guard.as_ptr()
    }
    pub(crate) fn data_ptr(this: &Self) -> *const T {
        unsafe { addr_of!((*Self::inner_ptr(this)).data) }
    }
}

impl<'a, T> Deref for Guard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*Self::data_ptr(self) }
    }
}

/// A replacement for [`std::sync::Arc`].
pub struct Arc<T> {
    pub(crate) ptr: NonNull<ArcInner<T>>,
    pub(crate) phantom: PhantomData<ArcInner<T>>,
}

/// Similar to [`std::sync::Arc`]. There is no weak count and thus no `Weak` struct.
/// In accordance with the deferred reclamation scheme, the ref count of the pointed-to block
/// may not immediately be decremented on drop.
impl<T> Arc<T> {
    /// See: [`std::sync::Arc::new`].
    pub fn new(data: T) -> Self {
        unsafe {
            let ptr = NonNull::new_unchecked(ArcInner::new(data));
            Self {
                ptr,
                phantom: PhantomData,
            }
        }
    }

    /// # Safety
    /// Since `Guard`s may exist, there is no safe `get_mut`.
    /// It is the user's responsibility to ensure that there are no other pointers to the same allocation.
    pub unsafe fn get_mut_unchecked(this: &mut Self) -> &mut T {
        &mut (*this.ptr.as_ptr()).data
    }

    /// # Safety
    /// See [`std::sync::Arc::from_raw`].
    pub unsafe fn from_raw(ptr: *const T) -> Self {
        Self {
            ptr: NonNull::new_unchecked(find_inner_ptr(ptr).cast_mut()),
            phantom: PhantomData,
        }
    }

    /// Returns the number of strong (`Arc` or `AtomicArc`) pointers to this allocation.
    pub fn ref_count(this: &Arc<T>) -> usize {
        unsafe { (*this.ptr.as_ptr()).ref_count.load(SeqCst) }
    }

    pub(crate) fn inner_ptr(this: &Self) -> *const ArcInner<T> {
        this.ptr.as_ptr()
    }
    pub(crate) fn data_ptr(this: &Self) -> *const T {
        unsafe { addr_of!((*Self::inner_ptr(this)).data) }
    }
}

impl<T> Clone for Arc<T> {
    fn clone(&self) -> Self {
        unsafe {
            ArcInner::increment(self.ptr.as_ptr());
        }
        Self {
            ptr: self.ptr,
            phantom: PhantomData,
        }
    }
}

impl<T> Deref for Arc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*Self::data_ptr(self) }
    }
}

impl<T> Drop for Arc<T> {
    fn drop(&mut self) {
        unsafe {
            ArcInner::delayed_decrement(self.ptr.as_ptr());
        }
    }
}

impl<'a, T> From<&Guard<'a, T>> for Arc<T> {
    fn from(value: &Guard<'a, T>) -> Self {
        unsafe {
            let inner_ptr = Guard::inner_ptr(value);
            _ = (*inner_ptr).ref_count.fetch_add(1, SeqCst);
            Self {
                ptr: NonNull::new_unchecked(inner_ptr.cast_mut()),
                phantom: PhantomData,
            }
        }
    }
}

impl<'a, T> From<&Arc<T>> for Guard<'a, T> {
    fn from(value: &Arc<T>) -> Self {
        let guard = CTX.with_borrow(|ctx| ctx.must_protect(value.ptr));
        Guard { guard }
    }
}

pub(crate) unsafe fn find_inner_ptr<T>(ptr: *const T) -> *const ArcInner<T> {
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
pub(crate) struct ArcInner<T> {
    pub(crate) birth_epoch: u64,
    pub(crate) ref_count: AtomicUsize,
    pub(crate) data: T,
}

impl<T> ArcInner<T> {
    pub(crate) fn new(data: T) -> *mut Self {
        Box::into_raw(Box::new(ArcInner {
            birth_epoch: RECLAIMER.current_epoch(),
            ref_count: AtomicUsize::new(1),
            data,
        }))
    }

    pub(crate) unsafe fn increment(ptr: *mut Self) {
        _ = (*ptr).ref_count.fetch_add(1, SeqCst);
    }

    pub(crate) unsafe fn delayed_decrement(ptr: *mut ArcInner<T>) {
        CTX.with_borrow(|ctx| {
            ctx.retire(
                ptr as *mut u8,
                Layout::new::<ArcInner<T>>(),
                Self::decrement,
                (*ptr).birth_epoch,
            );
        });
    }

    unsafe fn decrement(ptr: *mut u8, _layout: Layout) {
        let inner_ptr = ptr as *mut ArcInner<T>;
        if (*inner_ptr).ref_count.fetch_sub(1, SeqCst) == 1 {
            drop(Box::from_raw(inner_ptr));
        }
    }
}

impl<T> From<&Arc<T>> for NonNull<T> {
    fn from(value: &Arc<T>) -> Self {
        unsafe { NonNull::new_unchecked(Arc::data_ptr(value).cast_mut()) }
    }
}

impl<'a, T> From<&Guard<'a, T>> for NonNull<T> {
    fn from(value: &Guard<'a, T>) -> Self {
        unsafe { NonNull::new_unchecked(Guard::data_ptr(value).cast_mut()) }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Arc, Guard};

    #[test]
    fn basic_test() {
        let x = Arc::new(55usize);
        assert_eq!(*x, 55);
        assert_eq!(Arc::ref_count(&x), 1);
        let y = Guard::from(&x);
        assert_eq!(*x, *y);
        drop(x);
        assert_eq!(*y, 55);
        drop(y);
    }
}

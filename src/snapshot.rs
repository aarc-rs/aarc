use crate::shared_ptrs::{AsPtr, CloneFromRaw, TryCloneFromRaw};
use crate::smr::default_reclaimer::DefaultReclaimer;
use crate::smr::smr::{ProtectPtr, Release, Retire};
use crate::Arc;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::ptr::NonNull;


/*
*/

/*


impl<T> From<&Snapshot<T>> for AtomicArc<T> {
    fn from(value: &Snapshot<T>) -> Self {
        unsafe {
            Arc::increment_strong_count(value.ptr.cast_mut());
            Self {
                ptr: AtomicPtr::new(value.ptr.cast_mut()),
            }
        }
    }
}

impl<T> From<Option<&Snapshot<T>>> for AtomicArc<T> {
    fn from(value: Option<&Snapshot<T>>) -> Self {
        value.map_or(AtomicArc::default(), AtomicArc::from)
    }
}

impl<T> From<&Snapshot<T>> for Weak<T> {
    fn from(value: &Snapshot<T>) -> Self {
        unsafe { Arc::downgrade(&*ManuallyDrop::new(Arc::from_raw(value.ptr))) }
    }
}

impl<T> From<&Snapshot<T>> for AtomicWeak<T> {
    fn from(value: &Snapshot<T>) -> Self {
        Self {
            ptr: AtomicPtr::new(Weak::into_raw(Weak::from(value)).cast_mut()),
        }
    }
}

impl<T> From<Option<&Snapshot<T>>> for AtomicWeak<T> {
    fn from(value: Option<&Snapshot<T>>) -> Self {
        value.map_or(AtomicWeak::default(), AtomicWeak::from)
    }
}
*/

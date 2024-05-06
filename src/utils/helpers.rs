pub(crate) fn alloc_box_ptr<T>(item: T) -> *mut T {
    Box::into_raw(Box::new(item))
}

#[allow(clippy::semicolon_if_nothing_returned)]
pub(crate) unsafe fn dealloc_box_ptr<T>(ptr: *mut T) {
    drop(Box::from_raw(ptr))
}

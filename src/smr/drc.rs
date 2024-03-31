pub trait Protect {
    type Guard;
    fn protect() -> Self::Guard;
}

pub trait ProtectPtr {
    type Guard;
    fn protect_ptr(ptr: *mut u8) -> Self::Guard;
}

pub trait Retire {
    fn retire(ptr: *mut u8, f: fn(*mut u8));
}

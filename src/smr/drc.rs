use std::cell::Cell;

pub trait Protect {
    fn begin_critical_section(&self);
    fn end_critical_section(&self);
}

pub trait ProtectPtr {
    type ProtectionHandle: Release;
    fn protect_ptr(&self, ptr: *mut u8) -> &Self::ProtectionHandle;
}

pub trait Release {
    fn release(&self);
}

pub trait Retire {
    fn retire(&self, ptr: *mut u8, f: Box<dyn Fn(*mut u8)>);
    fn drop_flag(&self) -> &Cell<bool>;
}

pub trait ProvideGlobal {
    fn get_global() -> &'static Self;
}

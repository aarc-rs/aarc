pub trait Protect {
    fn begin_critical_section();
    fn end_critical_section();
}

pub trait ProtectPtr {
    type ProtectionHandle: 'static + Release;
    fn protect_ptr(ptr: *mut u8) -> &'static Self::ProtectionHandle;
}

pub trait Release {
    fn release(&self);
}

pub trait Retire {
    fn retire(ptr: *mut u8, f: Box<dyn Fn()>);
}

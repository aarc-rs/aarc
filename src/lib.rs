#![doc = include_str!("../README.md")]

pub use atomics::AtomicArc;
pub use atomics::AtomicWeak;
pub use smart_ptrs::Arc;
pub use smart_ptrs::AsPtr;
pub use smart_ptrs::Guard;
pub use smart_ptrs::RefCount;
pub use smart_ptrs::StrongPtr;
pub use smart_ptrs::Weak;

pub(crate) mod atomics;

pub(crate) mod smart_ptrs;

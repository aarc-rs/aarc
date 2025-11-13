#![doc = include_str!("../README.md")]

pub use atomics::AtomicArc;
pub use atomics::CompareExchange;
pub use smart_ptrs::Arc;
pub use smart_ptrs::Guard;

pub(crate) mod atomics;

pub(crate) mod smart_ptrs;

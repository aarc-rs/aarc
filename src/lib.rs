#![doc = include_str!("../README.md")]

pub use atomics::AtomicArc;
pub use atomics::AtomicWeak;
pub use smart_ptrs::Arc;
pub use smart_ptrs::AsPtr;
pub use smart_ptrs::SmartPtr;
pub use smart_ptrs::Snapshot;
pub use smart_ptrs::StrongPtr;
pub use smart_ptrs::Weak;

pub(crate) mod atomics;
pub(crate) mod smart_ptrs;

/// Traits and structs pertaining to safe memory reclamation.
pub mod smr {
    /// Traits pertaining to deferred reference counting.
    pub mod drc;

    /// The crate-default reclaimer.
    pub mod standard_reclaimer;
}

pub(crate) mod utils {
    pub(crate) mod helpers;
    pub(crate) mod spinlock;
    pub(crate) mod sticky_counter;
    pub(crate) mod unrolled_linked_list;
    pub(crate) mod unsafe_arc;
}

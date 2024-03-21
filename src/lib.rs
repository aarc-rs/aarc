#![doc = include_str!("../README.md")]

pub use atomics::AtomicArc;
pub use atomics::AtomicWeak;
pub use atomics::Shared;
pub use atomics::Strong;
pub use shared_ptrs::Arc;
pub use shared_ptrs::AsPtr;
pub use shared_ptrs::Snapshot;
pub use shared_ptrs::Weak;

pub(crate) mod atomics;
pub(crate) mod shared_ptrs;

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
    pub(crate) mod unrolled_linked_list;
    pub(crate) mod unsafe_arc;
}

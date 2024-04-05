# aarc

- [Quickstart](#quickstart)
- [Motivation](#motivation)
- [Examples](#examples)
- [Roadmap](#roadmap)
- [Resources](#resources)

### Quickstart

- [`AtomicArc`](https://docs.rs/aarc/latest/aarc/struct.AtomicArc.html) /
  [`AtomicWeak`](https://docs.rs/aarc/latest/aarc/struct.AtomicWeak.html): variants of `Arc` and
  `Weak` with atomically updatable pointers.
- [`Snapshot`](https://docs.rs/aarc/latest/aarc/struct.Snapshot.html): A novel smart pointer
  similar to a hazard pointer that significantly reduces contention when multiple threads load from
  the same `AtomicArc`. It prevents deallocation but does not contribute to reference counts.

### Motivation

Data structures built with `Arc` typically require locks for synchronization, as only
the reference counts may be atomically updated, not the pointer nor the contained data. While locks
are often the right approach, lock-free data structures can have better theoretical and practical
performance guarantees in highly-contended settings.

Instead of protecting in-place updates with locks, an alternative approach is to perform
copy-on-write updates by atomically installing pointers. To ensure that objects are not deallocated
while in use, mechanisms for safe memory reclamation (SMR) are typically utilized. However, classic
techniques like hazard pointers and epoch-based reclamation, and atomic shared pointer algorithms
like split reference counting, tend to scale poorly or require great care to use correctly.

`aarc` solves this problem by implementing a hybrid technique which combines the convenience of
reference counting with the efficiency of a state-of-the-art SMR backend. It provides several
distinct advantages:

* **Wait-freedom**: Reference counts are protected \[1, 2] by a mechanism based on Hyaline \[3, 4],
  a recently-introduced reclamation algorithm, rather than hazard pointers, EBR, RCU, locks, or
  split reference counting. Under typical conditions (i.e. there is a reasonable number of threads
  being spawned), all operations will be
  [wait-free](https://en.wikipedia.org/wiki/Non-blocking_algorithm#Wait-freedom), not merely
  lock-free.
* **Ease-of-use**: Many existing solutions based on the aforementioned algorithms require the user
  to manually protect particular pointers or pass around guard objects. This crate's APIs are
  ergonomic, designed for building lock-free data structures, and should feel familiar to Rust
  users. The atomics provided are compatible with the built-in `Arc`, and there are zero
  dependencies.

### Examples

Example 1: [Treiber Stack](https://en.wikipedia.org/wiki/Treiber_stack)

```rust no_run
use aarc::{AtomicArc, Snapshot};
use std::sync::Arc;

struct StackNode {
    val: usize,
    next: Option<Arc<Self>>,
}

struct Stack {
    top: AtomicArc<StackNode>,
}

impl Stack {
    fn push(&self, val: usize) {
        let mut top = self.top.load::<Arc<_>>();
        loop {
            let new_node = Arc::new(StackNode { val, next: top });
            match self
                .top
                .compare_exchange(new_node.next.as_ref(), Some(&new_node))
            {
                Ok(_) => break,
                Err(before) => top = before,
            }
        }
    }
    fn pop(&self) -> Option<Snapshot<StackNode>> {
        let mut top = self.top.load::<Snapshot<_>>();
        while let Some(top_node) = top.as_ref() {
            match self
                .top
                .compare_exchange(top.as_ref(), top_node.next.as_ref())
            {
                Ok(_) => return top,
                Err(actual_top) => top = actual_top,
            }
        }
        None
    }
}
```

### Roadmap

- [x] implement core algorithms
- [ ] implement misc. performance optimizations
- [ ] add tagged pointers
- [ ] add more tests and stabilize APIs
- [ ] add `no_std` support

### Resources

1. [Anderson, Daniel, et al. "Concurrent Deferred Reference Counting with Constant-Time Overhead."](https://dl.acm.org/doi/10.1145/3453483.3454060)
2. [Anderson, Daniel, et al. "Turning Manual Concurrent Memory Reclamation into Automatic Reference Counting."](https://dl.acm.org/doi/10.1145/3519939.3523730)
3. [Nikolaev, Ruslan, et al. "Snapshot-Free, Transparent, and Robust Memory Reclamation for Lock-Free Data Structures."](https://arxiv.org/abs/1905.07903)
4. [Nikolaev, Ruslan, et al. "Crystalline: Fast and Memory Efficient Wait-Free Reclamation"](https://arxiv.org/abs/2108.02763)

\* note that this crate's `Snapshot` has no relation to the concept of snapshots discussed in \[3];
rather, it is the snapshot pointer introduced by \[1].
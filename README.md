# aarc

- [Quickstart](#quickstart)
- [Motivation](#motivation)
- [Examples](#examples)

### Quickstart

- [`Arc`](https://docs.rs/aarc/latest/aarc/struct.Arc.html): a replacement for the standard library's `Arc`, but implemented with deferred reclamation semantics.
- [`AtomicArc`](https://docs.rs/aarc/latest/aarc/struct.AtomicArc.html): an `Arc` with an atomically updatable pointer. Supports standard atomic operations like 
  `compare_exchange`.
- [`Guard`](https://docs.rs/aarc/latest/aarc/struct.Guard.html): A special smart pointer that is loaded from `AtomicArc`. It is similar to `Arc` in that it prevents 
  deallocation, but it does not contribute to reference counts. This reduces contention when multiple threads operate on 
  the same variable.

### Motivation

Data structures built with `Arc` typically require locks for synchronization, as only
the reference counts may be atomically updated, not the pointer nor the contained data. While locks
are often the right approach, lock-free data structures can have better theoretical and practical
performance guarantees in highly-contended and/or read-heavy settings.

Instead of protecting in-place updates with locks, an alternative approach is to perform copy-on-write updates by
atomically installing pointers. To avoid use-afer-free, mechanisms for safe memory reclamation (SMR) are typically
utilized (i.e. hazard pointers, epoch-based reclamation). `aarc` uses the wait-free and robust algorithm provided by 
the [`fast-smr`](https://github.com/aarc-rs/fast-smr) crate and builds on top of it, hiding unsafety and providing
convenient RAII semantics through reference-counted pointers.

### Examples

Example 1: [Treiber Stack](https://en.wikipedia.org/wiki/Treiber_stack)

```rust no_run
use std::ptr::null;
use aarc::{Arc, AtomicArc, CompareExchange, Guard};

struct StackNode {
    val: usize,
    next: Option<Arc<Self>>,
}

struct Stack {
    top: AtomicArc<StackNode>,
}

impl Stack {
    fn push(&self, val: usize) {
        let mut top = self.top.load();
        loop {
            let next = top.as_ref().map(Arc::from);
            let new_node = Arc::new(StackNode { val, next });
            match self.top.compare_exchange(top.as_ref(), Some(&new_node)) {
                Ok(_) => break,
                Err(before) => top = before,
            }
        }
    }
    fn pop(&self) -> Option<Guard<'_, StackNode>> {
        let mut top = self.top.load();
        while let Some(top_node) = top.as_ref() {
            let next = top_node.next.as_ref();
            match self.top.compare_exchange(top.as_ref(), next) {
                Ok(_) => return top,
                Err(actual_top) => top = actual_top,
            }
        }
        None
    }
}
```

### Resources

1. [Anderson, Daniel, et al. "Concurrent Deferred Reference Counting with Constant-Time Overhead."](https://dl.acm.org/doi/10.1145/3453483.3454060)
2. [Anderson, Daniel, et al. "Turning Manual Concurrent Memory Reclamation into Automatic Reference Counting."](https://dl.acm.org/doi/10.1145/3519939.3523730)
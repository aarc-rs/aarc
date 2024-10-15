# aarc

- [Quickstart](#quickstart)
- [Motivation](#motivation)
- [Examples](#examples)
- [Roadmap](#roadmap)
- [Resources](#resources)

### Quickstart

- [`Arc`](https://docs.rs/aarc/latest/aarc/struct.Arc.html) /
  [`Weak`](https://docs.rs/aarc/latest/aarc/struct.Weak.html): drop-in replacements for the standard library's `Arc`
  and `Weak`, but implemented with deferred reclamation semantics.
- [`AtomicArc`](https://docs.rs/aarc/latest/aarc/struct.AtomicArc.html) /
  [`AtomicWeak`](https://docs.rs/aarc/latest/aarc/struct.AtomicWeak.html): variants of `Arc` and
  `Weak` with atomically updatable pointers, supporting standard atomic operations like `load` and `compare_exchange`.
- [`Guard`](https://docs.rs/aarc/latest/aarc/struct.Guard.html): A novel smart pointer that can be loaded from
  `AtomicArc` or `AtomicWeak`, designed to reduce contention when multiple threads operate on the same atomic variable.
  It prevents deallocation but does not contribute to reference counts. (This was renamed from `Snapshot` in an earlier
  version, to reduce confusion.)

### Motivation

Data structures built with `Arc` typically require locks for synchronization, as only
the reference counts may be atomically updated, not the pointer nor the contained data. While locks
are often the right approach, lock-free data structures can have better theoretical and practical
performance guarantees in highly-contended settings.

Instead of protecting in-place updates with locks, an alternative approach is to perform copy-on-write updates by
atomically installing pointers. To avoid use-afer-free, mechanisms for safe memory reclamation (SMR) are typically
utilized (i.e. hazard pointers, epoch-based reclamation). `aarc` uses the blazingly fast algorithm provided by the
[`fast-smr`](https://github.com/aarc-rs/fast-smr) crate and builds on top of it, hiding unsafety and providing
convenient RAII semantics through reference-counted pointers.

### Examples

Example 1: [Treiber Stack](https://en.wikipedia.org/wiki/Treiber_stack)

```rust no_run
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
            let top_ptr = top.as_ref().map_or(null(), AsPtr::as_ptr);
            let new_node = Arc::new(StackNode {
                val,
                next: top.as_ref().map(Arc::from),
            });
            match self.top.compare_exchange(top_ptr, Some(&new_node)) {
                Ok(()) => break,
                Err(before) => top = before,
            }
        }
    }
    fn pop(&self) -> Option<Guard<StackNode>> {
        let mut top = self.top.load();
        while let Some(top_node) = top.as_ref() {
            match self
                .top
                .compare_exchange(top_node.as_ptr(), top_node.next.as_ref())
            {
                Ok(()) => return top,
                Err(actual_top) => top = actual_top,
            }
        }
        None
    }
}
```

### Roadmap

- [ ] relax atomic orderings from SeqCst to Acq/Rel
- [ ] add tagged pointers
- [ ] add more tests and stabilize APIs

### Resources

1. [Anderson, Daniel, et al. "Concurrent Deferred Reference Counting with Constant-Time Overhead."](https://dl.acm.org/doi/10.1145/3453483.3454060)
2. [Anderson, Daniel, et al. "Turning Manual Concurrent Memory Reclamation into Automatic Reference Counting."](https://dl.acm.org/doi/10.1145/3519939.3523730)
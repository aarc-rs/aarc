# aarc

### Quickstart

Example 1: [Treiber Stack](https://en.wikipedia.org/wiki/Treiber_stack)

```rust no_run
use aarc::{Arc, AtomicArc, Snapshot};
use std::sync::atomic::Ordering::SeqCst;

struct StackNode {
    val: usize,
    next: Option<Arc<Self>>,
}

struct Stack {
    top: AtomicArc<StackNode>,
}

impl Stack {
    fn push(&self, val: usize) {
        let mut top = self.top.load::<Snapshot<_>>(SeqCst);
        loop {
            let new_node = Arc::new(StackNode {
                val,
                next: top.as_ref().map(Arc::from),
            });
            match self
                .top
                .compare_exchange(top.as_ref(), Some(&new_node), SeqCst, SeqCst)
            {
                Ok(_) => break,
                Err(before) => top = before,
            }
        }
    }
    fn pop(&self) -> Option<Arc<StackNode>> {
        let mut top = self.top.load::<Arc<_>>(SeqCst);
        while let Some(top_node) = top.as_ref() {
            match self
                .top
                .compare_exchange(top.as_ref(), top_node.next.as_ref(), SeqCst, SeqCst)
            {
                Ok(_) => return top,
                Err(actual_top) => top = actual_top,
            }
        }
        None
    }
}
```

### Motivation

Data structures built with `std::sync::Arc` typically require locks for synchronization. Despite 
having thread-safe reference counts, neither the pointer nor the contained data may be updated 
atomically. While locks are often the right approach, lock-free data structures can have better 
theoretical and practical performance guarantees in highly-contended settings.

Instead of protecting in-place updates with locks, an alternative approach is to perform 
copy-on-write updates by atomically installing pointers. To ensure that objects are not 
deallocated while in use, mechanisms for safe memory reclamation (SMR) are typically utilized. 
However, classic techniques like hazard pointers and epoch-based reclamation, and atomic shared 
pointer algorithms like split reference counting, tend to scale poorly or require great care to 
use correctly.

`aarc` solves this problem by implementing a hybrid technique which combines the convenience of
reference counting with the efficiency of a state-of-the-art SMR backend. This crate provides:

- `Arc` / `Weak`: slightly tweaked but functionally identical mirrors of `std::sync::Arc` and 
`std::sync::Weak`.
- `AtomicArc` / `AtomicWeak`: variants of `Arc` and `Weak` with support for atomic operations like 
`compare_exchange`.
- `Snapshot`\*: A novel `Arc`-like pointer that accelerates reads and writes to large data structures 
by orders of magnitude. It prevents deallocation but does not contribute to reference counts.

These structs have distinct advantages:

* **Wait-freedom**: Reference counts are protected \[1, 2] by a mechanism based on Hyaline \[3, 4], 
a recently-introduced reclamation algorithm, rather than hazard pointers, EBR, RCU, locks, or 
split reference counting. Under typical conditions (i.e. there is a reasonable number of threads 
being spawned), all operations will be 
[wait-free](https://en.wikipedia.org/wiki/Non-blocking_algorithm#Wait-freedom), not merely 
lock-free.
* **Decoupled**: Ideally, this crate would be compatible with `std::sync::Arc`. However, this is 
not possible without introducing inefficiencies due to the lack of fine-grained control over the 
reference counts and the `Drop` impl. The structs in this crate are optimized for performance, and 
crucially, they allow for the use of `Snapshot`. (There are also zero dependencies.)
* **Ease-of-use**: Many existing solutions based on the aforementioned algorithms require the user 
to manually protect particular pointers or pass around guard objects. This crate's APIs are 
ergonomic, designed for building lock-free data structures, and should feel familiar to Rust users 
experienced with `std::sync::Arc` and `AtomicPtr`.

\* note that this has no relation to the concept of snapshots discussed in \[3]; rather, it is the 
snapshot pointer introduced by \[1, 2].

### Resources

1. [Anderson, Daniel, et al. "Concurrent Deferred Reference Counting with Constant-Time Overhead."](https://dl.acm.org/doi/10.1145/3453483.3454060) 
2. [Anderson, Daniel, et al. "Turning Manual Concurrent Memory Reclamation into Automatic Reference Counting."](https://dl.acm.org/doi/10.1145/3519939.3523730)
3. [Nikolaev, Ruslan, et al. "Snapshot-Free, Transparent, and Robust Memory Reclamation for Lock-Free Data Structures."](https://arxiv.org/abs/1905.07903)
4. [Nikolaev, Ruslan, et al. "Crystalline: Fast and Memory Efficient Wait-Free Reclamation"](https://arxiv.org/abs/2108.02763)

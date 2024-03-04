# aarc

### Motivation

`std::sync::Arc` provides atomically reference-counted pointers. However, it is
[impossible](https://doc.rust-lang.org/std/sync/struct.Arc.html#thread-safety) to directly update 
the `Arc` variable itself or the object inside without using a mutex, which is suboptimal in 
certain performance-critical scenarios.

This crate provides `aarc::AtomicArc`, a struct that behaves like `Arc` in that it holds a pointer 
to and shares ownership of a heap-allocated object. Unlike `Arc`, `AtomicArc` supports atomic 
operations like `load`, `store`, and `compare_exchange` (thus it can be considered a cross between 
`Arc` and `std::sync::AtomicPtr`). `AtomicWeak` is also provided, corresponding to 
`std::sync::Weak`, which solves the problem of reference cycles.

`AtomicArc` offers distinct advantages:

* **Wait-freedom**: Reference counts are protected \[1, 2] by a mechanism based on two 
recently-introduced reclamation algorithms, Hyaline \[3] and Crystalline \[4], rather than 
hazard pointers, epochs, RCU, locks, or differential counting. Under typical conditions (i.e. there
is a reasonable number of threads being spawned), all operations will be 
[wait-free](https://en.wikipedia.org/wiki/Non-blocking_algorithm#Wait-freedom), rather than merely 
lock-free.
* **Ease-of-use**: Many existing solutions are incompatible with the standard library's `Arc` / 
`Weak`, do not support weak pointers, or require the user to manually protect particular pointers 
or pass around guard objects. `AtomicArc`'s APIs are ergonomic and designed for building lock-free 
data structures.

### Resources

1. [Anderson, Daniel, et al. "Concurrent Deferred Reference Counting with Constant-Time Overhead."](https://dl.acm.org/doi/10.1145/3453483.3454060) 
2. [Anderson, Daniel, et al. "Turning Manual Concurrent Memory Reclamation into Automatic Reference Counting."](https://dl.acm.org/doi/10.1145/3519939.3523730)
3. [Nikolaev, Ruslan, et al. "Snapshot-Free, Transparent, and Robust Memory Reclamation for Lock-Free Data Structures."](https://arxiv.org/abs/1905.07903)
4. [Nikolaev, Ruslan, et al. "Crystalline: Fast and Memory Efficient Wait-Free Reclamation"](https://arxiv.org/abs/2108.02763)

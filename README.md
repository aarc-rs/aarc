# aarc

### Motivation

Data structures built with `std::sync::Arc` typically require locks for synchronization. Despite 
having thread-safe reference counts, neither the pointer nor the contained data may be updated 
atomically. While locks are often the right approach, lock-free data structures can have better 
theoretical and practical performance guarantees in highly-contended settings.

Instead of protecting in-place updates with locks, an alternative approach is to use copy-on-write 
semantics and install updates by atomically swapping a pointer. To ensure that a previously 
pointed-to object is not deallocated while in use, a mechanism for safe memory reclamation (SMR) is 
typically implemented. However, classic SMR techniques like hazard pointers and epoch-based 
reclamation, and other approaches to atomic shared pointers like split reference counting, tend to 
scale poorly and/or increase the burden on the programmer.

`aarc` solves this problem by implementing a two-part technique, which retains the convenience of
reference counting with `Arc` and defers deallocation to an efficient SMR backend. This crate 
provides:

- `Arc` / `Weak`: functionally identical mirrors of `std::sync::Arc` and `std::sync::Weak`, but 
with a few key tweaks to facilitate interoperability with `AtomicArc` and `AtomicWeak`.
- `AtomicArc` / `AtomicWeak`: variants of `Arc` and `Weak` with support for atomic operations like 
`load`, `store`, and `compare_exchange`.
- `Snapshot`: A novel `Arc`-like pointer that accelerates reads and writes to large data structures 
by orders of magnitude. It prevents deallocation but does not contribute to reference counts. 

These structs have distinct advantages:

* **Wait-freedom**: Reference counts are protected \[1, 2] by a mechanism based on Hyaline \[3, 4], 
a recently-introduced reclamation algorithm, rather than hazard pointers, EBR, RCU, locks, or 
split counting. Under typical conditions (i.e. there is a reasonable number of threads being 
spawned), all operations will be 
[wait-free](https://en.wikipedia.org/wiki/Non-blocking_algorithm#Wait-freedom), not merely 
lock-free.
* **Decoupled**: Ideally, this crate would be compatible with `std::sync::Arc`. However, this is 
not possible without introducing inefficiencies due to the lack of fine-grained control over the 
reference counts and the `Drop` impl. The `Arc` in this crate is optimized for performance, and 
crucially, it allows for the use of `Snapshot`. (It also has zero dependencies.)
* **Ease-of-use**: Many existing solutions require the user to manually protect particular pointers 
or pass around guard objects. `AtomicArc`'s APIs are ergonomic, designed for building lock-free 
data structures, and should feel familiar to Rust users experienced with `std::sync::Arc` and 
`AtomicPtr`.

### Resources

1. [Anderson, Daniel, et al. "Concurrent Deferred Reference Counting with Constant-Time Overhead."](https://dl.acm.org/doi/10.1145/3453483.3454060) 
2. [Anderson, Daniel, et al. "Turning Manual Concurrent Memory Reclamation into Automatic Reference Counting."](https://dl.acm.org/doi/10.1145/3519939.3523730)
3. [Nikolaev, Ruslan, et al. "Snapshot-Free, Transparent, and Robust Memory Reclamation for Lock-Free Data Structures."](https://arxiv.org/abs/1905.07903)
4. [Nikolaev, Ruslan, et al. "Crystalline: Fast and Memory Efficient Wait-Free Reclamation"](https://arxiv.org/abs/2108.02763)

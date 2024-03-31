## v0.2.0 - 2024-03-31

* removed custom Arc and Weak implementations.
* removed memory ordering parameters from all methods due to potential UB if the user did not
  provide strict enough orderings.
* renamed marker traits to "SmartPtr" and "StrongPtr".
* fixed incorrect Send / Sync auto impls on atomics: they were previously omitted.
* fixed bug in compare_exchange methods: potential UB in the failure case.
* added support for multiple critical sections per thread (e.g. during signal handling).
* added thread-local handles to vacate slots automatically on exit.
* replaced boxed Fns with fn ptrs and cache to eliminate unnecessary allocations.
* changed smr trait methods to use RAII guards instead of functions.
* removed unnecessary Release trait.

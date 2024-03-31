## v0.2.0 - 2024-XX-XX

### Added

- thread-local handles to vacate slots automatically on exit.
- support for multiple critical sections per thread (e.g. during signal handling).

### Changed

- retire logic: use fn ptrs and cache instead of boxed Fns to eliminate unnecessary allocations.
- marker traits: renamed to "SmartPtr" and "StrongPtr".

### Removed

- custom Arc and Weak implementations.

### Fixed

- incorrect Send / Sync auto impls on atomics: they were previously omitted.
- memory ordering parameters: removed from all methods due to potential UB if the user did not
  provide strict enough orderings.
- bug in compare_exchange methods: potential UB in the failure case.
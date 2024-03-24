## v0.2.0 - 2024-XX-XX

### Added

- thread-local handles to vacate slots automatically on exit.
- sticky_counter: for efficient weak pointer upgrades.

### Changed

- retire logic: use fn and cache instead of boxed Fns to eliminate unnecessary allocations.
- retire logic: retire every decrement - slightly less efficient but more semantically correct and
  allows for wait-free upgrades.
- marker traits: renamed as "Strong" and "Shared" could be clearer.
- upgrade method on Weak: made generic, now can also upgrade to a Snapshot.

### Fixed

- incorrect Send / Sync auto impls on atomics: they were previously omitted.
- memory ordering parameters: removed from all methods due to potential UB if the user did not
  provide strict enough orderings.
- bug in compare_exchange methods: potential UB in the failure case.
## v0.2.0 - 2024-XX-XX

### Added

- thread-local handles to vacate slots automatically on exit.

### Changed

- retire logic: use fn and cache instead of boxed Fns to eliminate unnecessary allocations.
- memory ordering parameters: removed from all methods due to potential UB if the user did not
  provide strict enough orderings.

### Fixed

- incorrect Send / Sync auto blanket impls on atomics: they were previously omitted.
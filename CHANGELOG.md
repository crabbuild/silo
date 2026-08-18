# Changelog

All notable SILO changes will be recorded here.

## Unreleased

## 0.1.0 - 2026-08-17

### Added

- Extracted the immutable S3 version-control ledger from the [Prolly
  monorepo](https://github.com/crabbuild/prolly) into the standalone
  `silo-s3-core` and `silo-s3-client` crates.
- Added Prolly-backed snapshots, branches, object-version history, repository
  diffs, structural merges, recovery checkpoints, and bounded garbage
  collection.
- Added RustFS integration and provider qualification, strict CI, downstream
  compatibility checks, dependency security checks, and release packaging.
- Added million-file metadata verification for workloads containing 20-byte
  objects.

### Compatibility

- Preserved the `prolly-s3` durable protocol namespace for existing buckets.

# Releasing SILO

SILO is private and closed-distribution for the current release cycle. GitHub
tags produce a private release bundle; crates.io publishing is intentionally
disabled until the project is opened.

## Release checklist

1. Run the full CI checks from a clean checkout.
2. Run the RustFS qualification suite with the pinned image.
3. Reopen a repository created by the previous release and verify historical
   reads, branch refs, tags, listing cursors, and exact object versions.
4. Review `GA-CONTRACT.md`, `QUALIFICATION.md`, and the changelog.
5. Create and push an annotated tag such as `v0.1.0`.
6. Confirm the private GitHub release contains both crate archives and the
   commit identifier used for qualification.

## Future public release

When SILO is ready to open, add a protected crates.io publishing environment
using GitHub trusted publishing. Publish `silo-s3-core` before
`silo-s3-client`, wait for registry index propagation, and run a clean-clone
consumer build before announcing the release.

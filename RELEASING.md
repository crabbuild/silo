# Releasing SILO

SILO is open source under the MIT License. GitHub tags produce public release
bundles containing the validated crate archives. Crates.io publication remains
controlled separately until trusted publishing and the final compatibility
policy are enabled.

## Release checklist

1. Run the full CI checks from a clean checkout.
2. Run the RustFS qualification suite with the pinned image.
3. Reopen a repository created by the previous release and verify historical
   reads, branch refs, tags, listing cursors, and exact object versions.
4. Review `GA-CONTRACT.md`, `QUALIFICATION.md`, and the changelog.
5. Create and push an annotated tag such as `v0.1.0`.
6. Confirm the public GitHub release contains both crate archives and the
   commit identifier used for qualification.

## Publishing to crates.io

When crates.io publication is enabled, use a protected environment with GitHub
trusted publishing. Publish `silo-s3-core` before `silo-s3-client`, wait for
registry index propagation, and run a clean-clone consumer build before
announcing the release.

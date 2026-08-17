# SILO

SILO is an immutable version-control ledger layered over S3-compatible object
storage. It gives a bucket a durable history of object versions, commits,
branches, tags, listings, diffs, merges, recovery checkpoints, and garbage
collection while keeping user file bodies as complete immutable provider
objects.

> **Repository status:** SILO is currently a private, closed-distribution
> repository. The source inherits CrabBuild's MIT license, but no public crate
> or binary release is made by the current CI workflows.

## Capabilities

- whole-object `put`, `get`, delete, list, and historical reads;
- atomic multi-file commit sessions with durable checkpoints and resume;
- branches, tags, bounded logs, diffs, reflogs, and structural merges;
- resumable fsck, cross-provider repair, logical backup verification, and GC;
- branch-local writer fencing and concurrent publication across branches;
- node and payload caching, optional persistent Foyer cache, and telemetry;
- operation, journal, ref-catalog, and commit-graph indexes;
- RustFS compatibility tests plus explicit AWS qualification gates.

SILO deliberately does not pack or split user file bodies. One logical file is
one complete immutable payload object. Prolly metadata nodes may be packed, but
they never contain user payload bytes.

## Workspace

| Package | Purpose | Rust floor |
|---|---|---:|
| [`silo-s3-core`](core) | provider-independent ledger and durable format | 1.89 |
| [`silo-s3-client`](client) | AWS SDK-shaped S3 provider adapter | 1.94.1 |

The public client interface is `silo_s3_client::Client`.

## Quick start

SILO requires an S3 or S3-compatible bucket with versioning enabled, strong
read-after-write semantics, conditional writes, exact-version reads, and a
repository prefix reserved exclusively for SILO.

```toml
[dependencies]
silo-s3-client = { path = "../silo/client", default-features = false }
```

The complete client guide, including AWS setup and runnable Rust examples, is
in [`client/README.md`](client/README.md).

## Local RustFS

```bash
docker compose -f docker-compose.rustfs.yml up -d
scripts/run_rustfs_examples.sh
```

Use `SILO_RUSTFS_*` and `SILO_S3_*` environment variables for local settings.
The old `PROLLY_*` names are not required by the SILO repository; persisted
repository paths and wire identifiers retain `prolly-s3` for compatibility.

## Documentation

- [Client guide](client/README.md)
- [API guide](API.md)
- [SILO architecture](SILO-DESIGN.md)
- [Cache and scale design](CACHE-AND-SCALE-DESIGN.md)
- [Operations](OPERATIONS.md)
- [Qualification gates](QUALIFICATION.md)
- [GA contract](GA-CONTRACT.md)
- [Enterprise-readiness audit](ENTERPRISE-READINESS-AUDIT.md)
- [Durable path specification](spec/prolly-s3/paths.md)
- [State machines](spec/prolly-s3/state-machines.md)
- [Architecture decisions](docs/adr)

## Development

```bash
rustup show
cargo fmt --all -- --check
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
python3 spec/prolly-s3/conformance/verify.py
scripts/check_clean_downstream.sh
```

The RustFS provider suite is opt-in:

```bash
SILO_S3_RUSTFS=1 \
  cargo test --workspace -p silo-s3-client --test rustfs_repository -- --nocapture
```

AWS qualification is intentionally opt-in and requires isolated operator-owned
buckets. See [`QUALIFICATION.md`](QUALIFICATION.md) before running it.

## Compatibility promise

The SILO brand and package names are new, but persisted repository identity is
not. Domain-separated IDs, durable paths, canonical encoding, and the
`prolly-s3` protocol namespace remain stable so repositories created before
the extraction can be reopened by SILO.

## Security

Do not report a vulnerability in a public issue. See [`SECURITY.md`](SECURITY.md)
for the private reporting process.

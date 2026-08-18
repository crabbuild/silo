# SILO

SILO is an immutable version-control ledger layered over S3-compatible object
storage. It gives a bucket durable history for object versions, commits,
branches, tags, listings, diffs, merges, recovery checkpoints, and garbage
collection while keeping user file bodies as complete immutable provider
objects.

> **Repository status:** SILO is open source under the MIT License. The
> repository is pre-1.0; tagged releases publish public GitHub release bundles,
> while crates.io publication remains a separately controlled release step.

![SILO architecture](diagram/prolly-s3-architecture.svg)

## Why SILO

SILO combines a content-addressed payload plane with Prolly metadata trees and
an append-only publication journal:

- each distinct file body is stored as one complete immutable provider object;
  identical bodies may be reused by content hash;
- commits contain immutable snapshots and parent links, while branches are
  compare-and-swap protected mutable refs;
- commit sessions make multi-file ingestion atomic and durably resumable;
- branch-local writer authority fences stale writers, while independent
  branches can publish concurrently;
- indexes and caches are advisory and rebuildable; repository refs and linked
  publication events remain authoritative;
- fsck, repair, history transfer, backup verification, retention pins, and GC
  are bounded, checkpointed workflows.

SILO deliberately does not pack or split user file bodies. Prolly metadata nodes
may be packed, but they never contain user payload bytes. Larger or resumable
transfers are completed by an external provider transfer manager and then
handed to SILO for whole-object verification and publication.

## Capabilities

- whole-object `put`, `get`, delete, list, range reads, and historical reads;
- atomic multi-file commit sessions with durable checkpoints and resume;
- branches, tags, bounded logs, diffs, reflogs, and structural merges;
- resumable fsck, cross-provider repair, logical backup verification, and GC;
- branch-local writer fencing and concurrent publication across branches;
- node and payload caching, optional persistent Foyer cache, and telemetry;
- operation, journal, ref-catalog, and commit-graph indexes;
- RustFS compatibility tests plus explicit AWS qualification gates.

## Workspace

| Package | Purpose | Rust floor |
|---|---|---:|
| [`silo-s3-core`](core) | Provider-independent ledger and durable format | 1.94.1 |
| [`silo-s3-client`](client) | AWS SDK-shaped S3 provider adapter | 1.94.1 |

The workspace toolchain is pinned to Rust 1.94.1 in
[`rust-toolchain.toml`](rust-toolchain.toml). The application-facing type is
`silo_s3_client::Client`.

## Provider requirements

Initialize SILO only in a dedicated, versioned S3 or S3-compatible bucket (or
under a prefix reserved exclusively for SILO). The provider must support:

- conditional create and update writes;
- strong read-after-write and read-after-delete behavior for GET and LIST;
- paginated listings, physical-version listings, exact-version reads/deletes,
  and byte-range reads;
- a known per-key physical-version limit with enough control-record headroom;
- no lifecycle rule or default Object Lock retention that can mutate or retain
  repository control data unexpectedly.

The client runs provider qualification probes during initialization and stores a
signed, expiring capability attestation. Do not write, delete, or lifecycle
manage objects inside the reserved repository prefix outside SILO.

## Quick start

SILO is consumed from this workspace while crates.io publication is not yet
automated:

```toml
[dependencies]
silo-s3-client = { path = "../silo/client", default-features = false }
```

Create a client with the AWS SDK client and provider-attestation settings, then
initialize a new repository once. Reopen the same repository after a restart:

```rust
let client = silo_s3_client::Client::builder()
    .aws_client(aws)
    .bucket("my-versioned-bucket")
    .repository_prefix(".prolly")
    .default_branch("main")
    .writer("ingest-worker-01")
    // Configure provider_identity, attestation_signer, and the provider's
    // per-key version limit here; see client/README.md for the full example.
    .initialize()
    .await?;

let first = client
    .put_object("documents/readme.txt", b"first revision\n".to_vec())
    .await?;

let current = client
    .get_object("documents/readme.txt")
    .await?
    .expect("current object");
let historical = client
    .get_object_at(first.id, "documents/readme.txt")
    .await?
    .expect("historical object");

assert_eq!(current.bytes, historical.bytes);
```

The builder requires a provider identity, attestation signer, and explicit
per-key version-limit attestation in a real application. See the
[client guide](client/README.md) for a complete AWS configuration, batch
ingestion, branch, merge, restore, transfer, cache, and telemetry examples.

## Local RustFS

The repository includes a pinned RustFS service for local development and
integration tests. Set a local data directory explicitly if the compose-file
default is not appropriate for your machine:

```bash
export SILO_RUSTFS_DATA_DIR="$PWD/.data/rustfs"
docker compose -f docker-compose.rustfs.yml up -d

scripts/run_rustfs_examples.sh
```

Run the opt-in provider integration suite with:

```bash
SILO_S3_RUSTFS=1 \
  cargo test --workspace -p silo-s3-client \
  --test rustfs_repository -- --nocapture
```

The examples and tests use `SILO_RUSTFS_ENDPOINT`,
`SILO_RUSTFS_BUCKET`, `SILO_RUSTFS_ACCESS_KEY`, and
`SILO_RUSTFS_SECRET_KEY` for overrides. The legacy `PROLLY_*` names are
not required by this repository; persisted paths and wire identifiers retain
`prolly-s3` for compatibility.

## Feature flags

The client enables the persistent Foyer metadata cache by default:

```bash
# Minimal client build
cargo check -p silo-s3-client --no-default-features

# Default client build with Foyer
cargo check -p silo-s3-client --all-features
```

The optional `opentelemetry` feature exports client metrics through the
application-owned meter. See [the cache and scale design](CACHE-AND-SCALE-DESIGN.md)
and [the API guide](API.md) for production configuration.

## Development and qualification

Run the deterministic workspace checks from the repository root:

```bash
rustup show
cargo fmt --all -- --check
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
python3 spec/prolly-s3/conformance/verify.py
scripts/check_clean_downstream.sh
```

Dependency and security checks require `cargo-deny`:

```bash
scripts/check_dependency_security.sh
```

RustFS and AWS qualification tests are intentionally opt-in. AWS runs require
isolated operator-owned buckets and credentials; see the provider qualification
tests in [`client/tests`](client/tests) before running them.

## Documentation

- [Client guide](client/README.md)
- [API guide](API.md)
- [SILO architecture](SILO-DESIGN.md)
- [Cache and scale design](CACHE-AND-SCALE-DESIGN.md)
- [Enterprise-readiness audit](ENTERPRISE-READINESS-AUDIT.md)
- [Durable path specification](spec/prolly-s3/paths.md)
- [State machines](spec/prolly-s3/state-machines.md)
- [Architecture decisions](docs/adr)

## Compatibility promise

The SILO brand and package names are new, but persisted repository identity is
not. Domain-separated IDs, durable paths, canonical encoding, and the
`prolly-s3` protocol namespace remain stable so repositories created before
the extraction can be reopened by SILO.

Changes to canonical encoding, durable paths, or protocol identifiers require
a versioned compatibility decision and golden fixtures. See the
[contribution guide](CONTRIBUTING.md) for repository invariants.

## Contributing, releases, and license

SILO is maintained as an open-source CrabBuild repository. See
[`CONTRIBUTING.md`](CONTRIBUTING.md) for the pull-request checks and change
rules. Tagged release automation is defined in
[`.github/workflows/release.yml`](.github/workflows/release.yml).

The source is available under the [MIT License](LICENSE).

## Security

Do not report a vulnerability in a public issue. Use GitHub's private
vulnerability reporting for `crabbuild/silo` and do not include credentials,
provider endpoints, repository prefixes, or repository data in a report.

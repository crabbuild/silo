# silo-s3-core

`silo-s3-core` is the provider-independent repository engine behind
[SILO](https://github.com/crabbuild/silo). It stores immutable Prolly-tree
metadata over an object-store abstraction and provides durable snapshots,
logical object versions, commits, branches, tags, diffs, merges, recovery,
integrity checks, history transfer, and garbage collection.

The crate is published as version `0.1.0`:

- [crates.io](https://crates.io/crates/silo-s3-core)
- [docs.rs](https://docs.rs/silo-s3-core)
- [source repository](https://github.com/crabbuild/silo)

The core crate deliberately does not depend on the AWS SDK. A provider adapter
implements `ObjectPlane` and supplies the object-store semantics required by
the repository. The companion [`silo-s3-client`](https://crates.io/crates/silo-s3-client)
crate provides the AWS SDK-shaped application client and provider integration.

## What this crate is

The core is an immutable metadata and history engine. It owns:

- the canonical repository format and compatibility domain;
- Prolly state and version trees;
- content-addressed metadata node storage;
- whole-object payload bindings and logical object versions;
- commit creation, publication, branches, tags, and reflogs;
- branch-local writer authority and stale-writer fencing;
- bounded, restartable commit sessions;
- snapshot-bound listings, history, diffs, and merge plans;
- fsck, repair, history transfer, backup verification, and GC;
- advisory node, journal, operation, graph, and reference indexes; and
- stable error categories and retry guidance for provider adapters.

It is intended for an S3-compatible repository adapter, a test provider, or an
application that already has a suitable object-store boundary. The core does
not choose credentials, sign requests, qualify a bucket, export application
telemetry, or create an async runtime for its caller.

## What this crate is not

The core does not provide:

- an AWS SDK client or HTTP implementation;
- credential discovery, request signing, or provider attestation signing;
- a packed blob store for user file bodies;
- a chunked or multipart file format;
- provider-owned resumable-upload state;
- a global mutable journal head that requires namespace scans; or
- a guarantee that a particular provider, hardware profile, or workload meets
  a universal latency, cost, or throughput target.

SILO stores each distinct payload as one complete immutable provider object.
Prolly nodes contain metadata and indexes, never user file bytes. If a caller
needs multipart or resumable transfer, its provider transfer manager must
finish one complete object and hand that object to the repository for checksum
verification and publication.

## Architecture at a glance

```text
Application or silo-s3-client
             │
             │ repository operations
             ▼
      Repository<P: ObjectPlane>
       ┌─────────────────────────────┐
       │ Prolly state and version    │
       │ trees, commits, refs, DAG   │
       │ journals, indexes, jobs     │
       └──────────────┬──────────────┘
                      │ immutable and conditional object operations
                      ▼
             ObjectPlane adapter
                      │
                      ▼
             S3-compatible provider
```

There are two deliberately separate lanes:

| Lane | Core component | Stores | Publication role |
|---|---|---|---|
| Payload/data lane | `ImmutablePayloadStore` through `ObjectPlane` | One complete immutable object per distinct body | Prepared before the branch publication lane |
| Metadata lane | `Repository` and `ProllyObjectStore` | Prolly nodes, root manifests, commits, journals, refs, indexes, and job state | Ends with a conditional branch-ref CAS |

The separation is important for small-file workloads. Payload preparation and
hashing can be bounded and overlapped, while metadata mutation and the branch
compare-and-swap remain deterministic and serialized for one branch. The core
never hides payload request cost by packing user bodies into metadata nodes.

## Installation

Add the published crate to an adapter or low-level application:

```toml
[dependencies]
silo-s3-core = "0.1.0"
```

The core API is asynchronous. The embedding application supplies its Tokio or
compatible runtime and its concrete `ObjectPlane` implementation. Most
applications should use `silo-s3-client` unless they need to implement or
control the provider boundary themselves.

## Quick start with the in-memory provider

`MemoryObjectPlane` is a deterministic test double. It is useful for learning
the repository API and writing unit tests; it is not a durable or shared
production provider.

```rust
use std::{collections::BTreeMap, sync::Arc};

use silo_s3_core::{
    MemoryObjectPlane, ObjectHeaders, ProviderPerKeyVersionLimit, Repository,
    RepositoryOptions,
};

#[tokio::main]
async fn main() -> silo_s3_core::Result<()> {
    let plane = Arc::new(MemoryObjectPlane::new(true));
    let options = RepositoryOptions {
        repository_prefix: ".prolly".to_string(),
        default_branch: "main".to_string(),
        writer: "example-writer".to_string(),
        provider_per_key_version_limit: ProviderPerKeyVersionLimit::Finite(10_000),
        ..RepositoryOptions::default()
    };

    let repository = Repository::initialize(plane.clone(), options.clone()).await?;

    let receipt = repository
        .put_object(
            "main",
            b"documents/readme.txt".to_vec(),
            b"hello from SILO\n".to_vec(),
            ObjectHeaders::default(),
            BTreeMap::new(),
        )
        .await?;

    let current = repository
        .get_object("main", b"documents/readme.txt")
        .await?
        .expect("current object");
    assert_eq!(current.bytes, b"hello from SILO\n");

    // Reopen read-only using the same repository format and provider.
    let read_only = Repository::open(
        plane,
        RepositoryOptions {
            read_only: true,
            ..options
        },
    )
    .await?;
    let historical = read_only
        .get_object_at("main", receipt.id, b"documents/readme.txt")
        .await?
        .expect("historical object");
    assert_eq!(historical.bytes, b"hello from SILO\n");

    Ok(())
}
```

For a real provider, replace `MemoryObjectPlane` with an adapter that
implements `ObjectPlane`, qualify the bucket, and configure a finite or
unlimited per-key version profile. Provider versioning and control-record
headroom are part of the repository's safety contract.

## Repository options

`RepositoryOptions` is the complete construction and policy surface for the
core engine. The most important fields are:

| Option | Default | Purpose |
|---|---:|---|
| `repository_prefix` | `.prolly` | Reserved durable namespace in the bucket |
| `default_branch` | `main` | Branch opened and indexed by default |
| `writer` | `anonymous` | Stable writer identity recorded in authority and commits |
| `read_only` | `false` | Opens a handle without acquiring write authority |
| `limits` | `CanonicalLimits::default()` | Key, page, delete, commit, and object-size limits |
| `state_tree_format` | `TreeFormat::default()` | Canonical Prolly geometry for state trees |
| `authority_lease_millis` | `60_000` | Branch-authority lease duration |
| `provider_per_key_version_limit` | `Unknown` | Provider version headroom used during qualification |
| `node_cache` | in-memory cache | Optional verified immutable-node cache |
| `max_cached_node_pack_bytes` | 64 MiB | Packed metadata-object cache bound |
| `max_cached_node_locations` | 65,536 | Location-entry cache bound |
| `max_cached_node_bytes` | 64 MiB | Default node-body cache bound |
| `mutable_control_versions_to_retain` | 100 | Retained physical versions for mutable controls |
| `journal_index_max_unindexed_events` | 4,096 | Catch-up bound before index rebuild is required |
| `operation_index_max_unindexed_events` | 4,096 | Idempotency lookup tail bound |
| `idempotency_retention` | type default | Age/generation window for stable operation IDs |
| `clock` | `SystemClock` | Injectable time source for leases and tests |
| `ids` | `RandomIdSource` | Injectable operation and batch ID source |

The repository format records the tree geometry, canonical limits, provider
version profile, and idempotency policy at initialization. Reopening a
repository validates that the supplied options remain compatible with the
persisted format. Do not silently change canonical limits or tree geometry for
an existing repository.

`CanonicalLimits::default()` currently provides:

- maximum logical key length: 1,024 bytes;
- maximum list page: 1,000 entries;
- maximum delete batch: 1,000 keys;
- maximum mutations per commit: 10,000; and
- maximum logical object size: 5 TiB, subject to the provider's actual limits.

## The `ObjectPlane` contract

`ObjectPlane` is intentionally small, but its semantics are stronger than a
generic key/value store. The adapter must implement the following operations:

| Method | Required behavior |
|---|---|
| `get` | Read a current or exact physical version, optionally for a byte range |
| `head` | Read metadata and physical token without downloading the body |
| `put_immutable` | Create one immutable object, verify the expected SHA-256, and treat unequal existing bytes as corruption |
| `put_immutable_file` | Upload a complete file identity without requiring the whole body in memory |
| `transfer_immutable_from` | Transfer one complete object across planes without exposing multipart state to core |
| `load_mutable` | Read mutable control data and its physical token |
| `compare_exchange` | Conditionally create or replace a mutable record using an exact expected token |
| `list` | Paginate physical objects and, when requested, their physical versions |
| `delete_exact` | Delete only the exact physical version identified by the caller |
| `delete_exact_batch` | Optionally provide a provider-native bounded batch implementation |

The provider adapter is responsible for translating provider errors into the
core's `ErrorCode` and `RetryAdvice` categories while preserving provider
error codes, messages, and request IDs when available.

### Provider capabilities

Before initializing a production repository, the adapter must qualify the
bucket for:

- conditional create and conditional update;
- strong read-after-write behavior for `GET` and `LIST`;
- strong read-after-delete behavior for `GET` and `LIST`;
- paginated listings and byte-range reads;
- physical-version listings and exact-version reads/deletes;
- enabled provider versioning;
- a known single-object size and single-put limit; and
- enough per-key physical-version headroom for mutable control records.

The reserved repository prefix must be exclusive to SILO. External lifecycle
rules, default Object Lock retention, replication behavior, IAM permissions,
and provider version retention must be reviewed as part of qualification.

The core does not use namespace listing as a steady-state index lookup. A
provider list operation remains necessary for explicit GC, repair, cleanup, or
rebuild workflows, but ordinary reads and branch publication use authoritative
refs, linked publication events, and derived indexes.

## Durable data model

The repository separates logical history from physical provider objects.

### Payloads

An immutable payload path is derived from repository identity and the SHA-256
of the complete body:

```text
P/payloads/R/sha256/H0/H1/H
```

where `P` is the repository prefix, `R` is the repository identity, and `H`
is lowercase SHA-256 hex. The payload object is created once and then reused
when an identical body is written again in the same repository.

The logical version stores a `PayloadBinding` containing the derived path,
provider version or ETag token, and checksum. A payload binding is validated
against the repository identity and checksum before it is read or published.

Delete markers have no payload binding and do not create payload objects.

### Logical object versions

The current-object tree maps each logical key to its current `ObjectVersion`.
The version tree retains historical versions ordered by commit generation,
mutation ordinal, and version identity. A live version contains logical
headers, checksums, user metadata, tags, object size, and a payload binding. A
delete marker is a logical version with no body.

This is why a logical path can have a long history without consuming physical
versions on the original user key: the physical body lives at an immutable
content-derived path, while the logical key is represented in Prolly state.

### Commits and snapshots

Every commit contains:

- immutable parent commit IDs;
- root manifests for the current-object and version trees;
- a generation number;
- an inline or external transition delta;
- the author, message, operation identity, and authority stamp; and
- the publication metadata needed for recovery and indexing.

A `CommitId` identifies an immutable snapshot. Reads, listings, diffs, merge
plans, restore jobs, and verification jobs can all pin their work to an
explicit snapshot rather than observing a moving branch.

### Branches, tags, and publication events

A branch is a mutable compare-and-swap-protected reference. The reference
names an immutable publication event, and the event links to the commit and
the previous event. The branch ref CAS is the commit point for visibility.

Tags and retention pins are also durable references. The linked publication
journal acts as the immutable reflog and lets readers traverse branch movement
without scanning the entire commit namespace.

### Advisory indexes

The repository derives several rebuildable indexes from authoritative refs and
publication events:

- node locations for packed Prolly metadata;
- commit-graph entries and parent traversal hints;
- operation-index segments for bounded idempotency lookup;
- branch-local ref catalogs for branch and tag enumeration; and
- journal-derived maintenance state.

Indexes improve latency and request shape but are not repository truth. If an
index is missing, stale, or corrupt, the repository rebuilds or falls back to
deterministic content-addressed reads. It does not treat an index cache as a
second authority.

## Durable namespace

The exact path family is part of the `prolly-s3` compatibility format. The
following are the primary records under a configured prefix `P`:

```text
P/format/repository.cbor
P/format/initialization.cbor
P/authority/S/lease.cbor
P/refs/heads/N
P/refs/tags/N
P/payloads/R/sha256/H0/H1/H
P/commits/sha256/H0/H1/H
P/publications/sha256/H0/H1/H
P/staging/R/B/...
P/journal-index/...
P/operation-index/...
P/ref-catalog/...
P/administration/merge/E/...
P/administration/fsck/E/...
P/administration/gc/E/...
P/administration/physical-object-journal/R/...
P/gc/coordinator.cbor
P/gc/epochs/E/cursor.cbor
```

Immutable paths are create-once and content-addressed. Mutable control records
use exact physical-version compare-and-swap and retain a bounded number of
provider versions. There are no alternative path families or migration
namespaces.

See the complete [durable path specification](https://github.com/crabbuild/silo/blob/main/spec/prolly-s3/paths.md)
and [state-machine specification](https://github.com/crabbuild/silo/blob/main/spec/prolly-s3/state-machines.md)
before implementing a provider adapter or repository inspection tool.

## Writing objects

### Single-object writes

`Repository::put_object` is the simplest API. It validates the logical key,
hashes the body, prepares or reuses the immutable payload, updates the two
state trees, writes the commit metadata, and publishes the branch through one
conditional ref update.

Use `put_object_with_operation` when the caller owns a stable operation ID and
may retry an ambiguous response. Reusing the same operation ID with different
input is rejected with `ErrorCode::IdempotencyConflict`.

```rust
let receipt = repository
    .put_object(
        "main",
        b"reports/q3.parquet".to_vec(),
        parquet_bytes,
        ObjectHeaders::default(),
        user_metadata,
    )
    .await?;

println!(
    "published commit={} changed_keys={}",
    receipt.id, receipt.changed_keys
);
```

Single-object writes are convenient, but one call creates one commit. They are
not the preferred ingestion path for thousands or millions of independent
objects.

### Commit sessions and bulk metadata publication

Commit sessions upload complete payloads while staging and publish a group of
logical mutations with one branch CAS. This amortizes tree, commit, journal,
and ref work across the window:

```rust
let session = repository
    .begin_commit_session("main", "import 2026-08-17", 60_000)
    .await?;

let staged = repository
    .stage_commit_session_put_batch(
        &session,
        vec![
            (
                b"incoming/0001.json".to_vec(),
                first_body,
                ObjectHeaders::default(),
                BTreeMap::new(),
            ),
            (
                b"incoming/0002.json".to_vec(),
                second_body,
                ObjectHeaders::default(),
                BTreeMap::new(),
            ),
        ],
        32,
    )
    .await?;

let receipt = repository.publish_commit_session(session, staged).await?;
assert_eq!(receipt.changed_keys, 2);
```

`stage_commit_session_put_batch` performs bounded concurrent whole-object
preparation and returns mutations in deterministic key order. The ordered
metadata publication remains one operation on the branch. The default maximum
is 10,000 mutations per commit; choose smaller windows when provider request
budgets, retry time, or memory require it.

Durable sessions persist checkpoints so a process can resume after a crash:

1. Call `begin_durable_commit_session` and persist the returned
   `CommitSessionCheckpoint` or its `BatchId`.
2. Stage bounded windows with `stage_commit_session_put_batch`, or use the
   file and completed-object handoff methods.
3. Call `checkpoint_commit_session` after each durable window.
4. On restart, call `resume_commit_session(batch_id)`.
5. Publish the final canonical mutation set with
   `publish_commit_session`.

The original base commit must still be the branch head when the session
publishes. If the branch moved, publication returns `ErrorCode::BatchConflict`
and the caller must start a new session or reconcile the changes explicitly.

### Completed external objects

For large or resumable transfers, the provider transfer manager owns all
multipart state. Once the complete object exists, use
`stage_commit_session_existing_object` with its size and whole-object
SHA-256. The core verifies the binding and publishes only the final immutable
object. It never stores an upload ID, part list, part ETags, or chunk manifest.

## Reading, listing, and history

### Current and historical reads

Use `get_object` and `head_object` for the current branch head. Use
`get_object_at`, `head_object_at`, `list_objects_at`, and
`list_object_versions_at` for a fixed `CommitId`:

```rust
let head = repository.head("main").await?;

let current = repository
    .get_object("main", b"reports/q3.parquet")
    .await?;

let historical = repository
    .get_object_at("main", head, b"reports/q3.parquet")
    .await?;

let (_snapshot, versions) = repository
    .list_object_versions("main", b"reports/q3.parquet", 100)
    .await?;
```

The returned `ObjectData` includes the logical key, version metadata, complete
bytes, and the snapshot used for the read. `get_object_range` and
`head_object` avoid downloading more data than the caller needs, subject to
the provider adapter's range semantics.

### Snapshot-bound listings

`list_objects_page` returns a page plus an opaque continuation bound to the
repository, branch, prefix, and immutable snapshot. Pass that continuation
back unchanged; do not construct or edit it:

```rust
let mut continuation = None;
loop {
    let page = repository
        .list_objects_page("main", b"incoming/", continuation.as_deref(), 1_000)
        .await?;

    for object in &page.objects {
        println!("{}", String::from_utf8_lossy(&object.key));
    }

    continuation = page.continuation;
    if continuation.is_none() {
        break;
    }
}
```

Use `list_objects_page_at` when the caller already has a snapshot. Delimited
listing APIs return common prefixes for directory-style interfaces. Version
listing APIs traverse the historical version tree rather than the provider's
physical version list.

Continuation tokens are invalid if reused with a different repository,
branch, prefix, snapshot, or query. This prevents a moving branch from
silently changing the meaning of a resumed page.

### Commit history and reflogs

`log` is a convenient first-parent history call. `log_page_bounded` exposes a
constant-size `HistoryCursor` and explicit work, decoded-byte, and wall-clock
budgets for long histories. `open_reflog` and `read_reflog_page` traverse the
immutable publication journal newest-first.

For large or shared histories, use the bounded forms and persist cursors after
each page. Cursors are durable work descriptors, not in-memory iterators; a
new process can resume them after restart.

## Diffs and merges

### Diffs

`Repository::diff` returns all current-object changes between two snapshots.
For large snapshots, `diff_page_bounded` returns pages with a
`ObjectDiffCursor`. Direct parent-child diffs use the commit's exact ordered
transition stream. Non-adjacent snapshots use structural Prolly comparison
that reuses equal content-addressed subtrees.

The bounded page reports `compared_nodes` and `reused_subtrees`, which are
useful for metadata-amplification and sparse-change measurements.

```rust
let changes = repository.diff("main", older, newer).await?;
for change in changes {
    println!("{}", String::from_utf8_lossy(&change.key));
}
```

### Structural, restartable merges

Merges are durable jobs rather than one unbounded in-memory operation:

1. `start_merge` discovers merge bases with a bounded graph frontier.
2. `advance_merge` persists base discovery, structural diffs, conflicts, and
   output construction in job-scoped Prolly trees.
3. `merge_changes_page` and `merge_conflicts_page` expose the persisted plan.
4. `publish_merge` revalidates the target branch and publishes one two-parent
   commit through the target ref CAS.
5. `cleanup_merge` removes only the completed job's administration state.

The merge policy is explicit:

| Policy | Conflict behavior |
|---|---|
| `MergePolicy::Fail` | Persist conflicts and refuse publication until the caller chooses a resolved plan |
| `MergePolicy::Ours` | Keep the target branch value for conflicting keys |
| `MergePolicy::Theirs` | Select the source branch value for conflicting keys |

```rust
use silo_s3_core::{MergePhase, MergePolicy};

let mut merge = repository
    .start_merge(
        "main",
        "feature",
        None,
        MergePolicy::Fail,
        "merge feature",
    )
    .await?;

while merge.phase != MergePhase::ReadyToPublish {
    merge = repository.advance_merge(&merge, 1_000).await?.cursor;
}

let conflicts = repository
    .merge_conflicts_page(&merge, None, 1_000)
    .await?;
if conflicts.conflicts.is_empty() {
    let receipt = repository.publish_merge(&merge).await?;
    println!("merge commit={}", receipt.id);
}
```

The merge cursor is canonical and constant-sized. Persist it after every
`advance_merge` call. If the target branch moves before publication, the
operation fails rather than implicitly rebasing the plan.

## Branches, tags, authority, and recovery

### Branches and tags

`create_branch` creates a new mutable branch ref at an existing commit.
`create_tag` creates an immutable named reference to an existing commit.
`create_retention_pin` creates a tag-backed root that GC will retain. Branch
and tag catalog pages are driven by rebuildable sharded indexes.

Use fully qualified or otherwise unambiguous names in higher-level adapters.
Branch names are validated by `validate_branch`; malformed names fail before
provider writes.

### Writer authority and fencing

Writable repository handles acquire branch-local authority leases. Every
publication records an authority stamp containing the scope and generation.
Renewal and takeover use a branch-ref barrier so an old writer cannot continue
publishing after a successful takeover.

Independent branches can publish concurrently. Writers on one branch share a
single linear history and therefore contend on that branch's publication lane.
Provider or network failures that make a publication outcome ambiguous cause
the core to reconcile the operation ID before it fences the writer.

Use `start_shard_authority_maintenance` for a long-lived writable process so
branch authority renewal continues in the background. A fenced branch must be
explicitly taken over; blindly retrying a stale writer is not safe.

### Reflog reset and restore

`reset_branch` moves a branch to an existing commit only after checking the
expected current head and recording a reason in the publication journal.
`recover_branch` resolves a previous target through a selected reflog entry.

`start_restore` and `advance_restore` create fresh logical versions from an
older snapshot while preserving the branch's existing history. Restore work is
bounded and restartable; it does not rewrite or delete the original commit
DAG.

## Integrity, repair, and transfer

### fsck

`start_fsck` validates reachable commits, state trees, logical versions, node
closures, and payload bindings. Deep fsck also downloads and hashes physical
payload bytes. Each `advance_fsck` call writes a CAS-protected checkpoint
generation. A second process can resume the job with `resume_fsck`.

Only a completed job can enter `start_fsck_cleanup`. Cleanup exact-deletes
job-scoped payload and closure work objects, then removes checkpoint history in
bounded pages. Stale workers are rejected by checkpoint generation.

### Cross-repository repair

`start_repair_from` and `advance_repair_from` compare a source snapshot with a
destination snapshot, transfer verified complete payloads through the
`ObjectPlane` boundary, rebind them to the destination repository identity,
and remove destination-only logical keys. They do not copy provider version
IDs or assume that source and destination physical paths are interchangeable.

`start_backup_verification` and `advance_backup_verification` verify logical
objects and complete payload bytes between two repositories without changing
either snapshot.

### History transfer

The `start_history_transfer_from` family copies the full source commit DAG
parent-first and preserves merge topology. It creates destination-local commit
IDs and payload bindings, then publishes the imported history explicitly.
Commit IDs change because repository identity, operation identity, and provider
bindings change. Persist `HistoryTransferCursor` values across restarts and
use the returned mapping when recording source-to-destination relationships.

The older snapshot-only clone/fetch/push APIs preserve selected logical state
but not commit topology. Use the `history_` variants when history matters.

## Garbage collection

GC is a repository-wide, restartable mark-and-sweep workflow. It protects
everything reachable from live branches, tags, and retention pins, then
exact-deletes unreachable immutable commits, node packs, payloads, and
job-scoped work objects.

The workflow is driven by `GcCursor`:

1. `start_gc(grace_millis)` creates an epoch and closes publication admission.
2. `advance_gc` discovers roots, marks commits and nodes, and inventories
   candidates in bounded pages.
3. The coordinator catches up dirty roots and waits for publication tickets to
   drain before exposing the sweep phase.
4. `sweep_gc` exact-deletes candidates in bounded batches.
5. The cursor reaches `GcPhase::Complete` and publication admission reopens.

The grace period must exceed the longest time an upload, commit session, merge,
repair, or history transfer can remain unpublished. GC can be resumed after a
process restart with `resume_gc`.

`start_gc` uses compatibility-safe legacy candidate discovery. If every
payload writer uses journaled commit sessions, `start_gc_journaled` can use the
physical-object creation-intent journal instead of scanning the payload
namespace. Journal-only mode retains unjournaled or legacy payloads; use the
default legacy scan until all historical writers are covered.

Provider retention or legal hold may prevent exact deletion. Such objects are
reported as protected versions/bytes and remain physically present while the
GC epoch completes correctly.

Do not run an external lifecycle rule, bulk delete, or object mutation inside
the reserved prefix while GC or publication is active.

## Caching and metadata performance

The core's metadata path is designed to scale by keeping authoritative state
content-addressed, traversals bounded, and indexes rebuildable.

### Prolly metadata behavior

- State and version maps are Prolly trees addressed by immutable roots.
- Equal subtrees can be reused across snapshots and branches.
- Direct-child diffs can page the exact commit transition stream without
  comparing unchanged tree structure.
- Structural diffs and merges prune equal CIDs and traverse only changed
  frontiers.
- Large commit deltas move into immutable Prolly trees instead of growing the
  commit descriptor without bound.
- Branch-local journal indexes map CIDs and graph entries to packed node ranges.
- Missing indexes fall back to deterministic CID reads or bounded rebuilds;
  steady-state reads do not scan the entire commit or node namespace.
- Every long-running history, diff, merge, fsck, repair, transfer, and GC job
  uses a serializable cursor and explicit page/work limits.

### The million-small-object shape

For workloads containing very small bodies, the core metadata engine should be
used with a grouped publication strategy:

1. Hash and prepare complete payloads concurrently with bounded provider
   concurrency.
2. Stage a bounded commit-session window rather than publishing one commit per
   object.
3. Keep each window below `max_mutations_per_commit` and below provider request,
   timeout, and memory budgets.
4. Publish one metadata change set and one branch CAS per window.
5. Catch up the branch-local indexes after bulk publication or let the
   configured maintenance process advance them incrementally.
6. Traverse with snapshot-bound pages and continuation tokens.
7. Prewarm only roots and shared upper levels when startup latency matters;
   avoid a full snapshot prewarm unless the workload benefits from it.

With one million distinct 20-byte bodies, the payload plane still contains one
complete object for each distinct body. The core can reduce metadata
publication and traversal overhead, but it does not turn one million provider
objects into one packed blob. Identical bodies can reuse one content-addressed
payload object; unique bodies cannot.

### Node caches

`NodeCache` is a best-effort cache for verified immutable Prolly node bytes.
`MemoryNodeCache` provides a byte-bounded LRU implementation. A custom cache
may use the `NodeCache` trait for a persistent or hybrid tier.

Cache keys include repository identity, tree-format digest, and node CID. The
repository verifies returned bytes against the CID and treats cache errors,
corruption, eviction, and admission rejection as misses. Cache state is never
authoritative.

Use `Repository::node_cache_snapshot` and `NodeCacheSnapshot::delta_since` to
measure hits, misses, fetched bytes, avoided bytes, ranged fetches, prefetches,
pinned nodes, and byte amplification. `prewarm_node_cache` traverses a full
snapshot; `prewarm_node_cache_levels` loads only the root and a bounded number
of upper levels.

### Measuring the metadata lane

Measure on the actual provider, region, credentials, bucket configuration, and
workload that will run in production. Capture at least:

- provider operation counts and response bytes;
- commit and publication latency;
- branch CAS conflicts and authority fencing;
- metadata node requested and fetched bytes;
- cache hit ratio and byte amplification;
- index lag and rebuild work;
- list/diff/merge compared nodes and reused subtrees;
- checkpoint and restart time; and
- GC candidates, protected objects, deletes, and dirty-root restarts.

RustFS is useful for compatibility and regression testing. RustFS results do
not establish AWS latency, throttling, quotas, lifecycle, replication, or
cost behavior.

## Errors and retry behavior

Every fallible operation returns `silo_s3_core::Result<T>`, whose error is an
`Error` containing:

- a stable `ErrorCode`;
- `RetryAdvice` such as `Never`, `Safe`, `After`, `ReloadHead`, or
  `ReconcileOperation`;
- an optional operation ID;
- optional provider error code and message; and
- an optional provider request ID.

Important categories include:

| Error | Meaning |
|---|---|
| `RefConflict` | Branch or mutable control CAS lost; reload the current head |
| `BatchConflict` | A commit session's base branch moved before publication |
| `PreconditionFailed` | Authority, expected head, or writable-state precondition failed |
| `IdempotencyConflict` | A stable operation ID was reused with different input |
| `OutcomeUnknown` | Provider outcome is ambiguous; reconcile before retrying |
| `InvalidContinuationToken` | A cursor was used with a different query or snapshot |
| `MissingClosure` | An immutable commit, node, or payload required by history is missing |
| `ChecksumMismatch` / `CorruptContent` | Bytes or metadata do not match their recorded identity |
| `ProviderNotQualified` / `MissingCapability` | Provider contract is insufficient for the operation |
| `Throttled` / `Timeout` | Provider may allow a bounded retry according to advice |

Do not blindly retry every error. For `OutcomeUnknown`, reconcile the caller's
operation ID. For a branch conflict, reload the head and construct a new
mutation or merge plan. For a fenced writer, perform an explicit takeover
workflow rather than continuing with the stale handle.

## Compatibility and safety invariants

The core currently implements one canonical durable format in the
`prolly-s3` compatibility domain. It does not dual-write, migrate, or
negotiate alternative repository formats.

The following invariants are part of the persisted protocol:

- immutable records are content-addressed and verified after load;
- payload paths are repository-scoped and derived from complete-body SHA-256;
- delete markers contain no payload binding;
- a commit becomes visible only through a successful branch-ref CAS;
- publication events link branch history without a second mutable journal head;
- operation IDs cannot be reused with unequal input;
- authority generations fence stale writers;
- refs, commits, and linked publication events are authoritative;
- indexes and caches are rebuildable and may be discarded; and
- cleanup deletes exact physical versions rather than whichever version is
  currently visible.

Changes to canonical encoding, durable paths, tree format, protocol identity,
or version semantics require a compatibility decision and golden fixtures.
Existing repositories must be reopened with compatible `RepositoryOptions`.

## Operational limitations

- The repository prefix must be reserved exclusively for SILO.
- Every file must fit the repository and provider complete-object limits.
- The core does not split a file into repository-managed chunks.
- The built-in body path requires a complete `PutObject`-style provider write;
  larger or resumable transfers belong to an external transfer manager.
- Same-branch writes intentionally serialize at the publication CAS.
- Direct single-object writes create one commit each; use commit sessions for
  bulk ingestion.
- GC temporarily closes publication admission to preserve reachability
  correctness across processes.
- Provider-retained or legally held versions may remain after GC and are
  reported rather than force-deleted.
- Snapshot transfer preserves logical state; history transfer preserves commit
  topology but not source commit IDs or reflog identity.
- “Millions” and “billions” are workload descriptions, not guarantees. Quotas,
  request cost, throttling, latency, cache capacity, index lag, and hot-branch
  contention must be qualified for the target provider.

## Testing and development

From the repository root:

```bash
# Core unit and integration tests
cargo test --locked -p silo-s3-core

# All workspace features and packages
cargo test --locked --workspace --all-features

# Strict linting
cargo clippy --locked -p silo-s3-core --all-targets -- -D warnings

# Package verification
cargo package --locked -p silo-s3-core
```

The core test suite includes:

- canonical encoding and protocol fixtures;
- repository initialization, reopen, read-only access, and format checks;
- object versions, delete markers, ranges, listings, and snapshot cursors;
- atomic commit sessions, idempotent replay, checkpoints, and recovery;
- authority renewal, takeover, branch fencing, and concurrent branches;
- journal-derived node, graph, operation, and ref-catalog indexes;
- sparse structural diffs and restartable/replayable merges;
- fsck, repair, history transfer, backup verification, and exact-delete GC;
- immutable payload deduplication and checksum validation; and
- cache bounds, prewarming, prefetch, and metadata request accounting.

The expensive scale tests are opt-in. They should be run on a qualified
provider with an isolated bucket and explicit workload limits; they are not a
substitute for provider-specific production qualification.

## Public API map

| Type | Role |
|---|---|
| `Repository<P>` | High-level durable repository engine |
| `RepositoryOptions` | Format, authority, cache, index, and provider policy |
| `ObjectPlane` | Provider storage contract |
| `MemoryObjectPlane` | In-memory test provider |
| `ProllyObjectStore` | Prolly `AsyncStore` bridge and packed-node storage |
| `ImmutablePayloadStore` | Whole-object content-addressed payload operations |
| `NodeCache` / `MemoryNodeCache` | Verified immutable metadata-node cache |
| `ObjectVersion`, `PayloadBinding`, `BucketCommit` | Logical metadata and snapshot model |
| `CommitSessionCheckpoint` | Durable bulk-ingest checkpoint |
| `HistoryCursor`, `ObjectDiffCursor`, `MergeCursor` | Bounded traversal and job state |
| `FsckCursor`, `RepairCursor`, `HistoryTransferCursor`, `GcCursor` | Restartable maintenance state |
| `Error`, `ErrorCode`, `RetryAdvice` | Stable adapter-facing failure model |
| `TreeFormat`, `NodeLayoutSpec`, `ChunkingSpec` | Prolly metadata geometry |

See the generated [API documentation](https://docs.rs/silo-s3-core/0.1.0/silo_s3_core/)
for the complete public surface.

## Related documentation

- [SILO architecture](https://github.com/crabbuild/silo/blob/main/SILO-DESIGN.md)
- [Durable path specification](https://github.com/crabbuild/silo/blob/main/spec/prolly-s3/paths.md)
- [State machines](https://github.com/crabbuild/silo/blob/main/spec/prolly-s3/state-machines.md)
- [Client guide](https://github.com/crabbuild/silo/blob/main/client/README.md)
- [API guide](https://github.com/crabbuild/silo/blob/main/API.md)
- [Cache and scale design](https://github.com/crabbuild/silo/blob/main/CACHE-AND-SCALE-DESIGN.md)
- [Architecture decisions](https://github.com/crabbuild/silo/tree/main/docs/adr)
- [Prolly](https://github.com/crabbuild/prolly)

## License

SILO is available under the [MIT License](https://github.com/crabbuild/silo/blob/main/LICENSE).

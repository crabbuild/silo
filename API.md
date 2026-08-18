# Use the SILO client API

SILO is an asynchronous Rust client that adds immutable commits, branches, tags, historical reads, and restartable maintenance to an Amazon Simple Storage Service (S3)-compatible bucket. This reference explains the application-facing `silo_s3_client::Client` API in version `0.1.0`, shows the normal call sequence for each task, and links to complete examples that compile with the workspace.

Content type: Reference.

Goal: choose the right client method, understand the snapshot and consistency rules, and operate a repository safely after process or provider failures.

The client adapter uses the AWS SDK for Rust. The same adapter talks to Amazon S3 and compatible services that pass SILO’s provider qualification probes. The repository format, upgrade rules, cache contract, and production support posture are defined in [`GA-CONTRACT.md`](GA-CONTRACT.md).

## Choose an API by task

Use this table to find the shortest path to a common operation:

| Task | Start with | Result or follow-up |
| --- | --- | --- |
| Initialize a repository | `Client::builder().initialize()` | A writable `Client` attached to the default branch |
| Reopen a repository | `Client::builder().open()` | A client that reads the stored repository format and attestation |
| Write one file | `put_object` | One `CommitReceipt` and one new branch head |
| Retry an ambiguous write | `put_object_with_operation` | Reuse the same `OperationId` and identical input |
| Write several files atomically | `begin_commit` | Stage, checkpoint, then call `CommitSession::publish` |
| Ingest a collection | `put_objects` or `put_object_stream` | One receipt per durable published batch |
| Read the current file | `get_object` | `ObjectData` with bytes, version, and snapshot |
| Read an old file | `get_object_at` or a detached checkout | The bytes from an explicit `CommitId` |
| Read metadata only | `head_object` | `ObjectSummary` without downloading the payload |
| Read a byte range | `get_object_range` | An inclusive range from an explicit snapshot |
| List a prefix | `list_objects` | A page plus a snapshot ID and truncation flag |
| List with stable pagination | `list_objects_page` | An opaque continuation bound to one snapshot |
| Stream a large listing | `stream_objects` | A bounded-memory `Stream` of `ObjectSummary` values |
| Inspect object versions | `list_object_versions` or `list_versions_at` | Logical versions, including delete markers |
| Create an isolated line of work | `create_branch` and `checkout` | A writable branch handle |
| Name an immutable snapshot | `create_tag` | A detached checkout through `checkout` |
| Protect a snapshot from garbage collection (GC) | `create_retention_pin` | A durable garbage-collection root |
| Compare commits | `diff_bounded` | A bounded page of changed keys |
| Inspect first-parent history | `log_bounded` | A bounded page with a history cursor |
| Move a branch administratively | `reset_branch` | A compare-and-swap (CAS)-protected ref move recorded in the reflog |
| Restore old content forward | `start_restore` and `advance_restore` | New commits that preserve intervening history |
| Merge branches | `start_merge`, `advance_merge`, `publish_merge` | A durable structural merge job and receipt |
| Repair one logical snapshot | `start_repair_from` and `advance_repair_from` | A destination commit with source state |
| Preserve a commit directed acyclic graph (DAG) | `start_history_transfer_from` and `publish_history_transfer` | Destination-local commits with preserved parent topology |
| Verify a logical backup | `start_backup_verification` | A report that includes downloaded content checks |
| Check repository integrity | `start_fsck` and `advance_fsck` | A snapshot-bound, restartable `FsckReport` |
| Reclaim unreachable data | `start_gc`, `advance_gc`, `sweep_gc` | Bounded exact-version deletion |
| Inspect provider calls | `s3_operation_metrics` | Process-local S3 operation and byte counters |

## Prepare the storage provider

Create a dedicated bucket, or reserve a prefix that no other application writes. Enable bucket versioning before the first call to `initialize`. SILO stores immutable payloads, commit objects, mutable refs, indexes, and maintenance checkpoints under the repository prefix.

The provider must support all of these behaviors:

- conditional create and update writes;
- strong read-after-write behavior for `GET` and `LIST`;
- strong read-after-delete behavior for `LIST`;
- byte-range reads;
- paginated listings;
- physical-version listings and exact-version reads and deletes;
- a known per-key physical-version limit with control-record headroom.

Do not write, delete, or apply lifecycle rules to objects under the reserved prefix outside SILO. A lifecycle rule that can remove repository data, or a default Object Lock retention policy, causes provider qualification to fail. Give the client permission to read and write objects, issue conditional writes, read ranges, list physical versions, delete exact versions, and inspect bucket versioning during qualification.

SILO stores every distinct logical payload as one complete immutable provider object. It does not pack file bodies, split them into repository-managed chunks, or persist multipart-upload state. Prolly metadata nodes may be packed and range-read, but they never contain user file bytes.

### Add the client dependency

The repository is currently consumed from the workspace. A service that lives beside this checkout can use a path dependency:

```toml
[dependencies]
silo-s3-client = { path = "../silo/client" }
aws-config = "1.8"
aws-sdk-s3 = "1.140.0"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

The workspace pins Rust `1.94.1`. Use the same AWS SDK major and feature configuration as the client package when you share a lockfile.

## Create or reopen a client

The builder gathers provider identity, repository naming, writer identity, and optional cache and maintenance settings. `initialize` performs provider qualification and creates the repository format marker. Call it once for a new prefix. Use the same provider identity, signer trust, bucket, prefix, and stored format with `open` after a restart.

### Initialize an Amazon S3 repository

The AWS SDK loads credentials and region settings from the normal environment, profile, or workload identity chain. The example uses a hash-based message authentication code (HMAC) signer so it is self-contained; production deployments should keep signing keys in a protected key ring and rotate them through an implementation of `AttestationSigner`.

```rust
use std::sync::Arc;

use aws_config::BehaviorVersion;
use silo_s3_client::{
    core::ProviderPerKeyVersionLimit, Client, HmacAttestationSigner,
    ProviderIdentity,
};

async fn initialize_repository(bucket: &str) -> silo_s3_client::Result<Client> {
    let shared = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let aws = aws_sdk_s3::Client::new(&shared);
    Client::builder()
        .aws_client(aws)
        .bucket(bucket)
        .writer("ingest-worker-01")
        .provider_identity(ProviderIdentity::aws_region("us-west-2"))
        .attestation_signer(Arc::new(HmacAttestationSigner::single(
            "provider-key-2026-01", vec![0x41; 32],
        )?))
        .provider_per_key_version_limit(
            ProviderPerKeyVersionLimit::Finite(10_000),
        )
        .initialize()
        .await
}
```

For a compatible service, build an AWS SDK client with its endpoint and use `ProviderIdentity::s3_compatible`. Path-style addressing is enabled by default for that identity:

```rust
use aws_credential_types::Credentials;
use aws_types::region::Region;
use silo_s3_client::ProviderIdentity;

let endpoint = "http://127.0.0.1:9000";
let config = aws_sdk_s3::Config::builder()
    .behavior_version(aws_config::BehaviorVersion::latest())
    .region(Region::new("us-east-1"))
    .credentials_provider(Credentials::new(
        "access_key", "secret_key", None, None, "local-silo",
    ))
    .endpoint_url(endpoint)
    .force_path_style(true)
    .build();
let aws = aws_sdk_s3::Client::from_conf(config);
let identity = ProviderIdentity::s3_compatible(endpoint, "us-east-1");
```

The exact local RustFS bootstrap, including bucket versioning, lives in [`client/examples/common/mod.rs`](client/examples/common/mod.rs). The credentials in that file are local-demo values and are not suitable for a shared environment. Call `ProviderIdentity::path_style(false)` when a compatible service requires virtual-hosted addressing.

### Initialize versus reopen

| Method | Use it when | Important behavior |
| --- | --- | --- |
| `initialize()` | The prefix does not contain a SILO repository | Qualifies the provider, writes the attestation, and creates the canonical format |
| `open()` | The repository already exists | Loads and validates the stored attestation and format; it does not change tree geometry |

The builder requires these values before either method succeeds:

| Builder method | Why it matters |
| --- | --- |
| `aws_client` | Supplies the AWS SDK client used for all provider requests |
| `bucket` | Selects the versioned bucket |
| `provider_identity` | Binds the attestation to endpoint, region, addressing mode, and bucket class |
| `attestation_signer` | Verifies the signed provider capability attestation |
| `provider_per_key_version_limit` | Proves that mutable control keys have enough physical-version headroom |

The builder supplies these defaults when you omit them:

| Builder method | Default |
| --- | --- |
| `repository_prefix` | `.prolly` |
| `default_branch` | `main` |
| `writer` | `anonymous`; set a stable workload identity for writable services |
| `provider_attestation_validity` | 24 hours |
| `background_index_maintenance` | enabled |
| `read_only` | `false` |

`provider_attestation_validity` accepts values from 1 minute through 30 days. `provider_attestation` can select an existing attestation profile when several signed profiles are available. `ProviderPerKeyVersionLimit::Unknown` fails closed, because SILO cannot prove that mutable control keys will remain writable.

### Configure optional builder controls

Use these methods before `initialize` or `open`:

| Method | Use it for |
| --- | --- |
| `read_only(true)` | A reader, backup verifier, or upgrade check that must not publish |
| `authority_lease_duration` | The expiry window for branch writer authority |
| `state_tree_format` | Selecting the persisted metadata-tree geometry during initialization |
| `max_cached_node_pack_bytes` | Bounding in-process packed-node bodies |
| `max_cached_node_locations` | Bounding the packed-node location index |
| `max_cached_node_bytes` | Bounding in-process metadata-node bytes |
| `node_cache` | Supplying a custom `NodeCache` implementation |
| `production_cache_profile` | Enabling the supported persistent Foyer cache profile |
| `background_index_maintenance` | Enabling or disabling automatic journal-index catch-up |
| `journal_index_max_unindexed_events` | Setting the bounded journal-index lag threshold |
| `operation_index_limits` | Setting operation-index leaf, fan-out, and unindexed-event bounds |
| `mutable_control_version_retention` | Controlling retained provider versions for mutable control objects |
| `telemetry` | Sending bounded startup and interval metrics to an application sink |

The stored tree format is immutable. Supply the identical format when reopening an existing repository. If you need different metadata geometry, use a new prefix and transfer verified history into it.

## Understand repositories, commits, and snapshots

SILO separates mutable names from immutable state. A branch is a compare-and-swap protected name that points to one commit. A commit contains metadata-tree roots, parent IDs, object transitions, the writer identity, and an audit message. A `CommitId` identifies one immutable snapshot.

The client uses these terms throughout the API:

- **Repository**: One logical history under one bucket and reserved prefix. `repository_id()` returns its immutable identity.
- **Branch**: A writable ref such as `main` or `feature/search`. `head()` resolves the selected branch head unless the handle is detached.
- **Tag**: An immutable name for one commit. `tag()` reads it, and `create_tag()` creates it.
- **Snapshot**: The object and version state represented by one `CommitId`.
- **Logical object version**: A version record for one user key. It contains headers, checksums, metadata, and either a payload binding or a delete marker.
- **Payload binding**: The checksum-addressed physical provider object that stores the complete file body.
- **Operation ID**: A caller-controlled identity for reconciling an ambiguous publication result.
- **Batch ID**: The durable identity of a resumable commit session.

The branch ref write is the publication point. SILO can upload payloads, commit objects, metadata nodes, and journal records before that write succeeds. Those immutable candidates remain unreachable until a successful ref compare-and-swap makes the commit visible. Reads at an explicit `CommitId` remain stable while the branch advances.

`Client` implements `Clone`. Clones share repository state, node caches, writer authority maintenance, branch-index maintenance, telemetry, and S3 operation counters. A clone is a new handle with the same repository identity, not a new repository.

## Select branches, tags, and commits

`checkout` accepts a string, a typed `CheckoutRef`, or a `CommitId`. Unqualified strings resolve a branch before a tag. The `refs/heads/` and `refs/tags/` forms remove that ambiguity. A commit ID always creates a detached checkout.

```rust
use silo_s3_client::{CheckedOutRef, CheckoutRef};

let head = client.head().await?;
client.create_tag("release-2026.8", head).await?;

let feature = client.checkout("feature").await?;
assert_eq!(feature.branch(), Some("feature"));

let release = client.checkout("refs/tags/release-2026.8").await?;
assert_eq!(release.branch(), None);
assert_eq!(release.checked_out_ref().target(), Some(head));

let detached = client.checkout(CheckoutRef::Commit(head)).await?;
assert!(matches!(
    detached.checked_out_ref(),
    CheckedOutRef::Commit(id) if *id == head
));
```

Branch checkouts remain writable. Tag and commit checkouts are detached: reads use their immutable target, while object mutations, branch mutations, ref movement, and maintenance operations that require a branch return `InvalidRevision`.

Use these handle and revision methods to inspect the selected context:

| Method | Returns |
| --- | --- |
| `bucket()` | The physical bucket name |
| `branch()` | `Some(name)` for an attached branch, otherwise `None` |
| `checked_out_ref()` | The resolved `CheckedOutRef` enum |
| `repository_id()` | The immutable repository ID |
| `head()` | The selected commit, or the current attached branch head |
| `resolve_snapshot(snapshot)` | Validated state roots and commit metadata for a specific snapshot |
| `commit(id)` | One validated `BucketCommit` |

Create and remove branch names with expected-head checks:

```rust
let base = client.head().await?;
let feature = client.create_branch("feature", Some(base)).await?;

let feature_client = client.checkout("feature").await?;
let feature_head = feature_client
    .put_object("features/search.txt", b"enabled\n".to_vec())
    .await?
    .id;

client.delete_branch("feature", feature_head).await?;
println!("created {} at {}", feature.name, feature.target);
```

`create_branch(name, None)` starts at the currently selected attached branch head. `delete_branch` requires the exact expected target, so a concurrent update fails instead of deleting a branch that moved.

Manage immutable names and garbage-collection roots with these methods:

| Method | Behavior |
| --- | --- |
| `create_tag(name, target)` | Creates a tag at an explicit commit |
| `tag(name)` | Loads a tag without changing the client checkout |
| `delete_tag(name, expected)` | Deletes a tag only if it still targets `expected` |
| `create_retention_pin(name, target)` | Creates a durable GC root at a commit |
| `retention_pin(name)` | Loads one retention pin |
| `delete_retention_pin(name, expected)` | Removes a pin with an expected-target check |
| `list_retention_pins_page(cursor, limit)` | Lists pins with a bounded catalog cursor |
| `list_branch_catalog_page(cursor, limit)` | Lists derived branch catalog entries |
| `list_tag_catalog_page(cursor, limit)` | Lists derived tag catalog entries |

Retention pins are durable names backed by the same ref protocol as tags. Keep a pin while an audit, legal hold, backup, or long-running historical traversal needs a snapshot to survive garbage collection.

## Write objects

Use `put_object` for an interactive write and `put_object_with_metadata` when the logical object needs S3-shaped headers or user metadata. Each call creates a new logical version and publishes one commit on the selected branch.

| Method | Input | Notes |
| --- | --- | --- |
| `put_object(key, bytes)` | UTF-8 key and complete body | Uses default headers and empty user metadata |
| `put_object_with_metadata(key, bytes, headers, metadata)` | Complete body plus `ObjectHeaders` and `BTreeMap<String, String>` | Stores metadata in the logical version |
| `put_object_with_operation(key, bytes, operation)` | Complete body plus caller-stable `OperationId` | Reconciles an already-applied result after an ambiguous response; headers and metadata use defaults |
| `copy_object(source_snapshot, source_key, destination_key)` | Source snapshot and keys | Reuses the immutable payload binding without uploading the body again |
| `delete_object(key)` | One key | Publishes one delete marker |
| `delete_objects(keys)` | An iterator of keys | Publishes all delete markers in one atomic commit |

Logical keys must be nonempty UTF-8 strings, must not begin with the repository prefix, and must follow the physical path rules. The default canonical key limit is 1,024 bytes. A key that ends in `/`, contains an empty path component, or contains `..` is rejected.

### Write metadata

`ObjectHeaders` contains these optional fields:

| Field | Stored meaning |
| --- | --- |
| `content_type` | Media type such as `application/json` |
| `content_encoding` | Content encoding such as `gzip` |
| `content_language` | Content language |
| `content_disposition` | Download disposition |
| `cache_control` | Cache-control directive |
| `expires_at_millis` | Expiration timestamp in Unix milliseconds |

User metadata is a `BTreeMap<String, String>`. SILO preserves it in the logical version and returns it through `ObjectVersion.body.kind`.

```rust
use std::collections::BTreeMap;
use silo_s3_client::core::ObjectHeaders;

let mut metadata = BTreeMap::new();
metadata.insert("owner".to_string(), "finance".to_string());

let receipt = client
    .put_object_with_metadata(
        "reports/2026/summary.txt",
        b"revenue=42\nstatus=preliminary\n".to_vec(),
        ObjectHeaders {
            content_type: Some("text/plain; charset=utf-8".to_string()),
            ..ObjectHeaders::default()
        },
        metadata,
    )
    .await?;

println!("published {} changed {} keys", receipt.id, receipt.changed_keys);
```

### Read a commit receipt

`CommitReceipt` identifies the publication that made a write visible:

| Field | Meaning |
| --- | --- |
| `id` | The new immutable commit ID |
| `operation` | The operation identity used for the publication |
| `branch` | The branch that advanced |
| `parents` | Parent commit IDs; normal writes have one parent |
| `changed_keys` | Number of logical keys changed |
| `object_versions` | New logical object-version IDs |
| `idempotent_replay` | Whether the result came from reconciliation of an earlier publication |

For one-key writes, `changed_keys` is `1`. A multi-key commit has one receipt with one new branch head, so readers see either the old state or the complete new state.

### Reconcile an ambiguous write

An `OutcomeUnknown` error means the client cannot prove whether the provider accepted the publication. Persist the operation ID before sending a request that crosses a job boundary. On retry, provide the same key, identical bytes, and the same operation ID.

```rust
use silo_s3_client::core::{ErrorCode, OperationId};

let operation = OperationId::new();
let body = b"checkpointed payload\n".to_vec();

let result = client
    .put_object_with_operation(
        "jobs/2026-08/result.txt", body.clone(), operation,
    )
    .await;

let receipt = match result {
    Ok(receipt) => receipt,
    Err(error) if error.code == ErrorCode::OutcomeUnknown => client
        .put_object_with_operation(
            "jobs/2026-08/result.txt", body, operation,
        )
        .await?,
    Err(error) => return Err(error),
};
println!("commit={} replay={}", receipt.id, receipt.idempotent_replay);
```

Do not retry with different bytes, headers, metadata, or a new operation ID when the outcome is unknown. The repository compares the operation input digest and returns `IdempotencyConflict` if the same ID is reused for different logical input.

### Copy and delete

Copy from an explicit snapshot when a destination should reproduce a historical source. The destination receives a new logical version, but the immutable payload binding is reused.

```rust
let source_snapshot = client.head().await?;
client
    .copy_object(
        source_snapshot,
        "reports/2026/summary.txt",
        "archive/2026/summary.txt",
    )
    .await?;

let delete_receipt = client
    .delete_objects(["tmp/part-1", "tmp/part-2", "tmp/part-3"])
    .await?;
println!("deleted {} logical keys", delete_receipt.changed_keys);
```

Deletion removes the key from the current-object tree and records a delete marker in version history. Historical snapshots can still resolve the earlier live version until garbage collection removes the unreachable physical data.

## Read objects and metadata

Use `get_object` for the selected revision, `get_object_at` for an explicit commit, `head_object` for metadata-only reads, and `get_object_range` for an inclusive byte range.

| Method | Return value | Payload behavior |
| --- | --- | --- |
| `get_object(key)` | `Option<ObjectData>` | Downloads the complete body for the current selected snapshot |
| `get_object_at(snapshot, key)` | `Option<ObjectData>` | Downloads the complete body from `snapshot` |
| `head_object(key)` | `Option<(CommitId, ObjectSummary)>` | Reads logical metadata without the body |
| `get_object_range(snapshot, key, range)` | `Option<ObjectRangeData>` | Downloads only the inclusive range from `snapshot` |

`None` means the key is not present in the selected current-object tree. A historical version list can still contain a delete marker or earlier live versions.

```rust
use silo_s3_client::core::LogicalObjectVersionKind;

let first = client
    .put_object("documents/readme.txt", b"first revision\n".to_vec())
    .await?;

let metadata = client
    .head_object("documents/readme.txt")
    .await?
    .expect("metadata exists");
let range = client
    .get_object_range(first.id, "documents/readme.txt", 0..=4)
    .await?
    .expect("range source exists");
let current = client
    .get_object("documents/readme.txt")
    .await?
    .expect("current object exists");

assert_eq!(metadata.0, first.id);
assert_eq!(range.bytes, b"first");
assert_eq!(current.snapshot, first.id);
if let LogicalObjectVersionKind::Live { size, .. } = current.version.body.kind {
    println!("size={size}");
}
```

The range uses `RangeInclusive<u64>`. SILO rejects a reversed range or a range that starts at or beyond the object size. If the requested end exceeds the size, the client clips the end to the last byte.

The `ObjectData` fields are `key`, `version`, `bytes`, and `snapshot`. `ObjectRangeData` has the same fields plus the actual clipped `range`. `ObjectSummary` contains `key` and `version`.

### Inspect logical versions

`ObjectVersion` contains an ID, ordering and timestamp data, a `LogicalObjectVersionKind`, and an optional immutable payload binding. The kind is either:

- `Live`, with size, logical ETag, headers, checksums, user metadata, and tags;
- `DeleteMarker`, which has no payload binding.

Use `list_object_versions` for one key. Use `list_versions_prefix` and `list_versions_at` when you need versions across a logical key prefix or need a byte cursor.

```rust
let (snapshot, versions) = client
    .list_object_versions("documents/readme.txt", 100)
    .await?;

for version in versions {
    println!(
        "snapshot={} version={} created_at={}",
        snapshot, version.id, version.body.created_at_millis
    );
}
```

`list_object_versions` returns at most the requested page size. For prefix scans, the `VersionSummary.cursor` is an opaque encoded key. Pass the last cursor to `list_versions_at` with the same snapshot and prefix:

```rust
let (snapshot, first_page) = client
    .list_versions_prefix("documents/", 100)
    .await?;
let after = first_page.last().map(|item| item.cursor.as_slice());
let (second_page, truncated) = client
    .list_versions_at(snapshot, "documents/", after, 100)
    .await?;

println!("read {} more versions; truncated={truncated}", second_page.len());
```

## List objects and paginate safely

`list_objects` is the concise API. It returns `(snapshot, objects, truncated)`, where `snapshot` is the branch head used for that call. Passing the last key as `after` resumes a lexical scan, but a later call can observe a newer branch head. Use the cursor API when every page must describe one immutable snapshot.

```rust
let mut after: Option<String> = None;
loop {
    let (snapshot, page, truncated) = client
        .list_objects("incoming/", after.as_deref(), 1_000)
        .await?;

    for object in &page {
        println!(
            "snapshot={} key={}",
            snapshot,
            String::from_utf8_lossy(&object.key),
        );
    }

    after = page
        .last()
        .map(|object| String::from_utf8_lossy(&object.key).into_owned());
    if !truncated {
        break;
    }
}
```

Use `list_objects_page` when a branch can advance during a scan. Page one captures the current branch head. Each continuation is an opaque string bound to the repository, branch, prefix, and captured snapshot. Reusing a token with a different query returns `InvalidContinuationToken`.

```rust
let first = client.list_objects_page("incoming/", None, 500).await?;
let mut continuation = first.continuation;
let mut objects = first.objects;

while let Some(token) = continuation {
    let page = client
        .list_objects_page("incoming/", Some(&token), 500)
        .await?;
    objects.extend(page.objects);
    continuation = page.continuation;
}

println!("scanned {} objects at one snapshot", objects.len());
```

Hold a retention pin on `first.snapshot` when the scan can overlap garbage collection. The cursor is immutable, but GC can remove the snapshot’s closure if no branch, tag, or retention pin keeps it reachable.

For bounded-memory scans, `stream_objects(prefix, page_size)` fetches cursor pages lazily:

```rust
use futures_util::StreamExt;

let mut stream = client.stream_objects("incoming/", 500);
futures_util::pin_mut!(stream);
while let Some(object) = stream.next().await {
    let object = object?;
    process_key(&object.key).await?;
}
```

`list_objects_delimited(prefix, delimiter, after, limit)` returns `DelimitedObjectPage` with `snapshot`, leaf `objects`, `common_prefixes`, and `truncated`. Use it to present an S3-style folder view. `list_objects_at(snapshot, prefix, after, limit)` performs an explicit snapshot scan without an opaque cursor.

## Publish several objects atomically

Use `begin_commit` when a set of changes must become visible together. Payloads upload while you stage them. Durable sessions checkpoint staged mutations so another process can resume after a crash.

### Use a durable commit session

`begin_commit()` returns a `CommitSessionBuilder`. Configure `message`, `expires_after`, `checkpoint_every`, or `ephemeral`, then call `start()`.

```rust
let mut session = client
    .begin_commit()
    .message("publish one consistent daily data set")
    .expires_after(std::time::Duration::from_secs(3_600))
    .checkpoint_every(256)
    .start()
    .await?;

let batch_id = session.id();
session
    .put_object("daily/customers.csv", b"id,name\n1,Ada\n".to_vec())
    .await?;
session
    .put_object("daily/orders.csv", b"id,total\n10,42\n".to_vec())
    .await?;
session.delete_object("daily/obsolete.csv")?;
session.checkpoint().await?;

let receipt = session.publish().await?;
println!("batch={batch_id} commit={}", receipt.id);
```

The session exposes these inspection methods:

| Method | Meaning |
| --- | --- |
| `id()` | The durable `BatchId` to persist for recovery |
| `operation()` | The session’s operation identity |
| `base_commit()` | The branch head captured when the session started |
| `staged_objects()` | Number of currently staged logical keys |
| `is_durable()` | Whether remote checkpoints and resume are enabled |

Call `checkpoint` before acknowledging upstream progress. A final checkpoint runs automatically before `publish`. `abort` records a durable abort for a durable session. Immutable payload candidates remain deduplicated and bounded cleanup removes expired session state.

### Resume after process loss

Persist the `BatchId` before reporting a source offset as complete. A replacement process can reopen the same branch and resume the last canonical checkpoint:

```rust
let mut session = client.resume_commit(batch_id).await?;
println!("resumed {} staged keys", session.staged_objects());

session
    .put_object("daily/events.ndjson", b"{\"event\":\"created\"}\n".to_vec())
    .await?;

let receipt = session.publish().await?;
println!("resumed commit={}", receipt.id);
```

Verified immutable payload bindings are reused during resume. The client does not upload a body again when its content-addressed payload already exists.

Use `.ephemeral()` only when the source can replay the entire job. An ephemeral session removes remote checkpoint requests and cannot resume staged metadata after process loss.

### Stream a commit session body

`CommitSession::put_stream` and `put_stream_with_metadata` accept an AWS `ByteStream`. SILO writes the stream to one bounded temporary-file spool, calculates checksums, uploads one complete immutable object, and removes the spool after staging.

```rust
use aws_sdk_s3::primitives::ByteStream;

let mut session = client.begin_commit().message("stream files").start().await?;
session
    .put_stream(
        "daily/events.ndjson",
        ByteStream::from_static(b"{\"event\":\"created\"}\n"),
    )
    .await?;
let receipt = session.publish().await?;
println!("published {}", receipt.id);
```

The stream path still creates one whole provider object. It does not implement repository-managed chunking or resumable upload state.

### Bulk-load a collection

`put_objects` is the in-memory bulk path. It accepts `Vec<PutObjectInput>` and a batch size, uploads with bounded concurrency, and returns one receipt per published batch.

```rust
use silo_s3_client::PutObjectInput;

let receipts = client
    .put_objects(
        vec![
            PutObjectInput {
                key: "incoming/0001.json".into(),
                bytes: br#"{"id":1}"#.to_vec(),
                headers: Default::default(),
                user_metadata: Default::default(),
            },
        ],
        1_000,
    )
    .await?;

println!("published {} batch(es)", receipts.len());
```

For an unbounded or fallible source, use `put_object_stream` with `BulkWriteOptions`:

```rust
use silo_s3_client::{BulkWriteOptions, PutObjectInput};

let options = BulkWriteOptions {
    batch_size: 10_000,
    concurrency: 32,
    checkpoint_every: 1_000,
};
let receipts = client.put_object_stream(incoming_objects, options).await?;
```

The stream item type is `silo_s3_client::Result<PutObjectInput>`. The client fills a checkpoint window, stages all payloads in that window, checkpoints the durable session, and continues until the batch or source ends. If the source or one object fails, the error includes the resumable batch ID in `operation_id` when a session was created.

`BulkWriteOptions` must use positive values. `batch_size` cannot exceed the repository’s canonical mutations-per-commit limit. `concurrency` cannot exceed 1,024, and `checkpoint_every` cannot exceed `batch_size`.

### Group independent callers with an ordered queue

`ordered_publication_queue` applies bounded backpressure while a worker prepares complete objects concurrently and publishes unique keys in deterministic order. Producers wait when `queue_capacity` is full. Repeated submissions for one key are split across consecutive commits so version order remains visible.

```rust
use silo_s3_client::OrderedPublicationOptions;

let queue = client.ordered_publication_queue(OrderedPublicationOptions {
    max_group_size: 1_000,
    upload_concurrency: 64,
    max_wait: std::time::Duration::from_millis(2),
    queue_capacity: 10_000,
    checkpoint_every: 1_000,
    durable: true,
})?;

let receipt = queue
    .put_object("events/0001.json", br#"{"ok":true}"#.to_vec())
    .await?;
println!("commit={} group={}", receipt.commit, receipt.group_size);
println!("remaining={}", queue.remaining_capacity());
```

`OrderedPublicationReceipt` contains the grouped `commit`, grouped `operation`, and `group_size`. The acknowledgement arrives only after the grouped branch CAS succeeds. Use `put_object_with_metadata` on the queue when the submission needs headers or user metadata. The queue worker stops after all handles are dropped.

### Hand off a large upload to an external transfer manager

The built-in `put_object`, session stream, and bulk paths use one provider `PutObject`. For a body larger than the provider single-PUT limit, or when the application owns multipart and resume behavior, use `prepare_external_object_upload` and `stage_external_object_upload`.

```rust
use md5::{Digest as _, Md5};
use sha2::Sha256;

let body = load_complete_body_from_external_storage().await?;
let sha256: [u8; 32] = Sha256::digest(&body).into();
let md5: [u8; 16] = Md5::digest(&body).into();
let handoff = session
    .prepare_external_object_upload(
        "large/archive.bin", body.len() as u64, sha256, md5,
    )
    .await?;
```

Upload one complete object to `handoff.path` with the provider transfer manager. Preserve the `prolly-sha256` metadata value as the lowercase hex SHA-256. Then pass the same handoff back to SILO:

```rust
session
    .stage_external_object_upload(
        &handoff,
        silo_s3_client::core::ObjectHeaders::default(),
        std::collections::BTreeMap::new(),
    )
    .await?;
let receipt = session.publish().await?;
```

SILO verifies the completed whole object, size, checksum, provider token, and handoff path before publication. It never stores upload IDs, parts, part ETags, or a chunk manifest.

### Remove expired commit sessions

An operator can call `cleanup_expired_commit_sessions(continuation, limit)` from an attached writable or read-only maintenance handle. The returned `CommitSessionCleanupReport` contains bounded progress and the next continuation. Repeat until the continuation is absent.

## Inspect history and compare snapshots

History APIs read immutable commits and never change the selected branch. `log` follows first-parent history. `diff` compares logical current-object entries between two commits. Use bounded variants for repositories where the result or traversal work can exceed one request budget.

| Method | Behavior |
| --- | --- |
| `log(limit)` | Returns up to `limit` first-parent commits from the selected revision |
| `log_bounded(start, cursor, limit, budget)` | Returns commits plus a constant-size `HistoryCursor` and budget evidence |
| `diff(from, to)` | Returns all changed logical keys in a convenience vector |
| `diff_bounded(from, to, cursor, limit)` | Returns a page, opaque cursor, compared-node count, and reused-subtree count |
| `commit(id)` | Loads one validated commit descriptor |

`TraversalBudget` bounds `max_commits`, `max_decoded_bytes`, and `max_elapsed`. Its defaults are 10,000 commits, 64 MB of decoded commit data, and 30 seconds.

```rust
use silo_s3_client::core::TraversalBudget;

let head = client.head().await?;
let history = client
    .log_bounded(head, None, 100, TraversalBudget::default())
    .await?;

for (id, commit) in &history.commits {
    println!("{} {:?}", id, commit.message);
}

if let Some(next) = history.continuation.as_ref() {
    let next_page = client
        .log_bounded(head, Some(next), 100, TraversalBudget::default())
        .await?;
    println!("next page has {} commits", next_page.commits.len());
}
```

Structural diff pages prune identical Prolly subtrees. The `compared_nodes` and `reused_subtrees` fields help explain metadata work without downloading user payloads.

### Read the reflog and move a branch

`open_reflog` captures a stable immutable journal view. `read_reflog_page` reads entries newest first. `reset_branch` performs a CAS-protected administrative move and records the reason. `recover_branch` selects a previous target from a reflog event and creates another auditable move.

```rust
let expected = client.head().await?;
let reflog = client.open_reflog().await?;
let page = client.read_reflog_page(&reflog, 100).await?;
let previous = page
    .entries
    .iter()
    .find_map(|entry| entry.event.old_target)
    .expect("a previous target exists");

let moved = client
    .reset_branch(previous, expected, "remove a bad publication")
    .await?;
println!("{} moved to {}", moved.branch, moved.new_target);
```

The ref move methods require an attached branch and the expected current target. A concurrent branch update returns `RefConflict` or `PreconditionFailed`; reload the head and decide whether the administrative action is still valid.

### Restore old state as new history

Use `start_restore(source, expected_head, message)` when you want to restore the content represented by an old snapshot while keeping all later commits in the branch history. The operation creates new logical versions and may publish multiple commits when the diff exceeds one commit’s canonical mutation limit.

```rust
let known_good = client
    .put_object("service/config.json", br#"{"mode":"safe"}"#.to_vec())
    .await?
    .id;
let unwanted = client
    .put_object("service/config.json", br#"{"mode":"broken"}"#.to_vec())
    .await?
    .id;

let mut restore = client
    .start_restore(known_good, unwanted, "restore known-good configuration")
    .await?;
let mut restored_commit = None;
while !restore.complete {
    let page = client.advance_restore(&restore, 1_000).await?;
    restored_commit = page
        .receipt
        .map(|receipt| receipt.id)
        .or(restored_commit);
    restore = page.cursor;
}
println!("restored to {:?}", restored_commit);
```

Persist `RestoreCursor` after each page. A cursor from another repository, branch, or expected head cannot be applied.

## Merge branches

SILO uses a durable structural three-way merge. The target is the client’s attached branch, and the source is the branch name passed to `start_merge`.

`MergePolicy` controls conflicting keys:

| Policy | Conflict behavior |
| --- | --- |
| `Fail` | Persist conflicts and refuse publication |
| `Ours` | Keep the target branch value |
| `Theirs` | Select the source branch value |

`start_merge` returns a `MergeCursor`. Persist it after every `advance_merge` page. If several best bases exist, read `merge_bases_page` and call `select_merge_base`. Inspect planned changes and conflicts with their corresponding page methods. Only `publish_merge` advances the target branch.

```rust
use silo_s3_client::core::{MergePhase, MergePolicy};

let mut merge = client
    .start_merge("feature", None, MergePolicy::Fail, "merge feature")
    .await?;

while merge.phase != MergePhase::ReadyToPublish {
    merge = client.advance_merge(&merge, 1_000).await?.cursor;
}

let changes = client.merge_changes_page(&merge, None, 1_000).await?;
let conflicts = client.merge_conflicts_page(&merge, None, 1_000).await?;
if !conflicts.conflicts.is_empty() {
    return Err(silo_s3_client::Error::new(
        silo_s3_client::ErrorCode::MergeConflict,
        "merge has unresolved conflicts",
    ));
}

let receipt = client.publish_merge(&merge).await?;
println!("merged {} keys", receipt.changed_keys);
```

The merge cursor stores the graph frontier, planned changes, conflicts, and output roots in immutable repository data. It remains constant-size even when the merge plan is large. Call `cleanup_merge(cursor, continuation, limit)` after publication or abandonment to remove job-scoped plan data in bounded pages.

## Repair, clone, fetch, push, and backup

SILO exposes two transfer families. Select the family based on whether the destination needs one logical snapshot or the source commit graph.

| Family | Methods | Preserves |
| --- | --- | --- |
| Snapshot synchronization | `start_repair_from`, `advance_repair_from`, plus `start_clone_from`, `start_fetch_from`, and `start_push_to` aliases | The selected logical objects and metadata; it publishes a destination-local commit |
| History synchronization | `start_history_transfer_from`, `advance_history_transfer_from`, `publish_history_transfer`, plus `history_` clone/fetch/push aliases | Parent topology, including merges, with destination-local commit and object-version IDs |

The history aliases are `start_history_clone_from`, `start_history_fetch_from`, and `start_history_push_to`. They all use the same restartable history-transfer cursor.

Snapshot repair copies source payloads, removes destination-only logical keys, and publishes when the bounded job completes. It does not copy provider version IDs. Use `start_backup_verification` afterward when the destination must be checked against the source.

```rust
let source_snapshot = source.head().await?;
let destination_head = destination.head().await?;
let mut repair = destination
    .start_repair_from(
        &source,
        source_snapshot,
        destination_head,
        "repair from primary",
    )
    .await?;

while repair.phase != silo_s3_client::core::RepairPhase::Complete {
    repair = destination
        .advance_repair_from(&source, &repair, 1_000)
        .await?
        .cursor;
}

println!("copied {} objects", repair.report.copied_objects);
```

History transfer walks the source commit closure parent-first, maps source commits to new destination IDs, and preserves merge parents. Publish only after `HistoryTransferCursor.complete` is true:

```rust
let source_head = source.head().await?;
let destination_head = destination.head().await?;
let mut transfer = destination
    .start_history_clone_from(&source, source_head, destination_head)
    .await?;

while !transfer.complete {
    transfer = destination
        .advance_history_transfer_from(&source, &transfer, 1_000)
        .await?
        .cursor;
}

let mapped_head = transfer.mapped_head.expect("source head was mapped");
destination
    .publish_history_transfer(&transfer, "publish imported history")
    .await?;
```

Use `history_transfer_mapping(cursor, source_commit)` to resolve one source commit after the transfer. History transfer preserves the source commit directed acyclic graph (DAG), including merge parents. Destination IDs change because the repository identity, authority stamps, and payload bindings are destination-local.

Backup verification compares logical keys, versions, and downloaded content between two snapshots. It does not trust only provider ETags or provider version IDs:

```rust
let mut verification = source
    .start_backup_verification(&destination, source_head, mapped_head)
    .await?;
while !verification.complete {
    verification = source
        .advance_backup_verification(&destination, &verification, 1_000)
        .await?
        .cursor;
}
println!(
    "verified {} objects and {} bytes",
    verification.report.objects_verified,
    verification.report.content_bytes_verified,
);
```

## Run integrity checks

`start_fsck(deep)` creates a durable, snapshot-bound integrity job. Metadata mode validates commits, trees, versions, node packs, and provider metadata. Deep mode also downloads and hashes each distinct reachable complete payload.

```rust
use silo_s3_client::core::FsckPhase;

let mut fsck = client.start_fsck(true).await?;
let job = fsck.job;
while fsck.phase != FsckPhase::Complete {
    fsck = client.advance_fsck(&fsck, 1_000).await?.cursor;
}

println!(
    "commits={} payloads={} bytes={}",
    fsck.report.commits,
    fsck.report.payloads_verified,
    fsck.report.deep_content_bytes_verified,
);
```

Persist `FsckCursor` after every page. If a process stops, reopen the repository, call `resume_fsck(job)`, and continue from the returned cursor. A stale worker that continues an older generation is fenced. After retaining the completed report for audit, start bounded cleanup:

```rust
use silo_s3_client::core::FsckCleanupPhase;

let mut cleanup = client.start_fsck_cleanup(job).await?;
while cleanup.phase != FsckCleanupPhase::Complete {
    cleanup = client
        .advance_fsck_cleanup(&cleanup, 1_000)
        .await?
        .cursor;
}
```

Cleanup exact-deletes the job’s payload-dedup work tree, commit-closure work tree, checkpoint history, and final checkpoint. It is allowed only after the fsck job completes.

## Reclaim unreachable data with GC

GC marks reachable commits, versions, nodes, and complete payloads, then sweeps unreachable immutable physical versions in bounded batches. Branches, tags, and retention pins are roots.

```rust
use silo_s3_client::core::GcPhase;

let grace_millis = 2 * 60 * 60 * 1_000;
let mut gc = client.start_gc(grace_millis).await?;

while gc.phase != GcPhase::Complete {
    gc = match gc.phase {
        GcPhase::Ready | GcPhase::Sweeping => {
            client.sweep_gc(&gc, 1_000).await?.cursor
        }
        _ => client.advance_gc(&gc, 1_000).await?.cursor,
    };
    // Persist `gc` after every page in a maintenance worker.
}

println!("deleted {} physical versions", gc.report.deleted_versions);
```

Set the grace period longer than the longest possible unpublished upload, durable commit session, merge, restore, repair, or transfer. GC closes repository-wide publication admission while it establishes its safety epoch. New publications can receive `PreconditionFailed` until cleanup reopens admission. Do not manually delete repository objects.

`start_gc_journaled` uses immutable ingest-window journals for payload candidate discovery. It is appropriate after all writers use the journaled bulk paths. Direct or legacy writers remain retained by that mode, so use `start_gc` during migration. `resume_gc`, `abandon_gc`, and `abandon_incomplete_gc` manage interrupted maintenance jobs.

Provider retention or legal hold can prevent exact deletion. GC records protected versions and bytes in its report and continues bounded cleanup; those physical versions remain in the bucket.

## Maintain indexes and writer authority

Derived indexes and caches improve read cost but are not the repository’s source of truth. Refs and linked publication events remain authoritative.

### Branch-index methods

| Method | Purpose |
| --- | --- |
| `advance_branch_indexes()` | Catch up the selected branch’s journal and operation indexes |
| `branch_index_health()` | Inspect target, indexed generation, lag, readiness, and last error |
| `wait_for_branch_indexes(timeout)` | Wait until the selected branch index is ready or return a timeout/error |
| `start_branch_index_rebuild()` | Capture a stable journal cursor for a rebuild |
| `advance_branch_index_rebuild(cursor, max_events)` | Rebuild journal-derived index state in bounded pages |
| `start_operation_index_rebuild(journal)` | Build operation reconciliation state from the same journal snapshot |
| `advance_operation_index_rebuild(cursor, max_events)` | Advance the operation-index rebuild |
| `cleanup_branch_index_rebuild(journal, operations, limit)` | Remove rebuild work state after installation |
| `repair_branch_catalog_page(continuation, limit)` | Repair the derived branch catalog by scanning physical refs |
| `repair_tag_catalog_page(continuation, limit)` | Repair the derived tag catalog by scanning physical refs |
| `cleanup_expired_commit_sessions(continuation, limit)` | Remove expired durable staging checkpoints |

Background branch-index maintenance is enabled by default and runs on a bounded interval. Disable it only for isolated request-shape measurement or a deliberately controlled maintenance process. A branch index that is behind can increase latency, but it does not change committed truth.

### Fencing and takeover

Writable clients renew branch authority in the background. If a process is fenced, stop its writes before starting a takeover. `fenced_branches()` reports branches this process can no longer publish. `takeover_branch_writer` requires the branch, expected writer, expected generation, and handoff evidence:

```rust
let new_generation = client
    .takeover_branch_writer(
        "main",
        "ingest-worker-01",
        expected_generation,
        "worker isolated by deployment controller",
    )
    .await?;

println!("main authority generation={new_generation}");
println!("fenced={:?}", client.fenced_branches()?);
```

Treat takeover as an operational fencing action. Isolate, revoke, or terminate the previous writer before supplying expected identity and generation. A takeover does not make a still-running old process safe.

## Configure and inspect the metadata cache

The `foyer-cache` feature is enabled by default. Persistent Foyer storage is an advisory cache for immutable Prolly nodes. Deleting the cache preserves correctness and can increase cold-read latency.

Use `ProductionCacheProfile` when you know the expected live object cardinality. The profile derives memory, disk, node-location, and prewarm bounds together:

```rust
use silo_s3_client::{Client, ProductionCacheProfile};

let profile = ProductionCacheProfile::new(
    "./prolly-node-cache",
    1_000_000,
)
.startup_prewarm(3, std::time::Duration::from_secs(30))
.require_successful_prewarm(true);

let client = Client::builder()
    // Add the required AWS client, bucket, provider, signer, and limit here.
    .production_cache_profile(profile)
    .open()
    .await?;

let startup = client.startup_metrics();
println!("open={} ms", startup.total_open_millis);
```

The profile requires one filesystem owner for its directory. Call `close_production_cache` during graceful shutdown after stopping request traffic and dropping other client clones. This method exists when the `foyer-cache` feature is enabled.

Call `CacheSizingRecommendation::for_object_count(expected_objects)` when you need the derived capacities without opening a client. `production_metadata_tree_format()` returns the encoded-byte-bounded tree format used when a new repository initializes through a production cache profile. `SupportedEnvelope::for_deployment(provider, object_count)` reports whether the published support posture is a controlled pilot, requires qualification, or has a failed performance gate.

For custom sizing, `FoyerNodeCache::open` accepts a `FoyerNodeCacheConfig` with memory capacity, disk capacity, disk block size, shard count, and a directory. Use one cache instance per filesystem owner and share its `Arc` within that process. The cache rejects entries that do not fit its configured block and the repository falls back to provider reads.

Use these methods and types to inspect performance:

| API | What it measures |
| --- | --- |
| `node_cache_snapshot()` | Hits, misses, insertions, errors, corruptions, waits, ranges, bytes, prefetch, pinning, and admission rejections |
| `prewarm_node_cache(snapshot)` | Traverses both state trees for a complete snapshot prewarm |
| `prewarm_node_cache_levels(snapshot, levels)` | Loads only bounded shared upper levels |
| `startup_metrics()` | Open, index catch-up, prewarm, cache, and provider activity |
| `performance_snapshot()` | A combined cache and provider counter snapshot |
| `ClientPerformanceSnapshot::delta_since(earlier)` | Counters for one measurement interval |
| `metadata_download_amplification()` | Provider response bytes per requested metadata-node byte |
| `provider_capabilities()` | The signed provider profile, including versioning and size limits |

The cache is never authoritative. SILO verifies cached node bytes against their content IDs before use and treats cache errors as misses.

## Inspect provider and wire metrics

`s3_operation_metrics()` returns process-local counters for adapter-level calls. It includes `GET`, `HEAD`, `PUT`, list, delete, and body-byte totals. SDK-internal retries do not increase these execution counters. `reset_s3_operation_metrics()` returns the current counters and resets them atomically.

The `S3OperationMetrics` fields are `get_object`, `head_object`, `put_object`, `list_objects_v2`, `list_object_versions`, `delete_object`, `delete_objects`, `uploaded_body_bytes`, and `downloaded_body_bytes`. Call `total_calls()` for the sum of operation counters or `delta_since(earlier)` for one measurement interval.

```rust
let before = client.s3_operation_metrics();
let _ = client.list_objects("incoming/", None, 100).await?;
let after = client.s3_operation_metrics();
let interval = after.delta_since(before);

println!(
    "list_calls={} downloaded={} bytes",
    interval.list_objects_v2,
    interval.downloaded_body_bytes,
);
```

For SDK execution and HTTP-attempt counts, attach `S3WireAttemptInterceptor` while constructing the AWS SDK client. The adapter cannot attach it after client construction:

```rust
use silo_s3_client::S3WireAttemptInterceptor;
use aws_config::BehaviorVersion;

let wire = S3WireAttemptInterceptor::new();
let config = aws_sdk_s3::Config::builder()
    .behavior_version(BehaviorVersion::latest())
    .region(aws_types::region::Region::new("us-west-2"))
    .interceptor(wire.clone())
    .build();
let aws = aws_sdk_s3::Client::from_conf(config);

let metrics = wire.metrics();
println!("retries={}", metrics.retry_transmissions());
```

Read or reset wire counters only across a quiescent measurement interval. `retry_transmissions()` is `transmissions - executions`, bounded at zero.

### Export OpenTelemetry metrics

Enable the `opentelemetry` feature and provide the application-owned meter. The application owns the meter provider, exporter, resource attributes, sampling, and shutdown:

```rust
use std::{sync::Arc, time::Duration};
use opentelemetry::global;
use silo_s3_client::{Client, OpenTelemetryClientMetrics};

let sink = OpenTelemetryClientMetrics::new(global::meter("silo"));
let client = Client::builder()
    // Add the required AWS and repository settings here.
    .telemetry(sink, Duration::from_secs(15))
    .open()
    .await?;
```

The built-in sink exports bounded-dimension cache, metadata-byte, provider-operation, provider-byte, startup-duration, prewarm, and cache-error instruments. The names and reference alerts are listed in [`GA-CONTRACT.md`](GA-CONTRACT.md).

## Handle errors and retries

Every fallible client method returns `silo_s3_client::Result<T>`, which is an alias for `Result<T, silo_s3_client::Error>`. `Error` exposes:

| Field | Meaning |
| --- | --- |
| `code` | Stable `ErrorCode` category |
| `retry` | `RetryAdvice` such as `Never`, `Safe`, `After`, `ReloadHead`, or `ReconcileOperation` |
| `message` | Human-readable local diagnostic |
| `operation_id` | Operation or batch identity when available |
| `provider_code` | Provider error code, if the SDK supplied one |
| `provider_message` | Provider diagnostic, if available |
| `provider_request_id` | Provider request ID for support and tracing |

Handle the retry category as part of the error contract:

```rust
use silo_s3_client::core::{ErrorCode, RetryAdvice};

match client.get_object("incoming/result.json").await {
    Ok(Some(object)) => consume(object.bytes),
    Ok(None) => report_missing(),
    Err(error) => match error.retry {
        RetryAdvice::Safe => retry_read(),
        RetryAdvice::After(delay) => schedule_retry(delay),
        RetryAdvice::ReloadHead => reload_head_and_replan(),
        RetryAdvice::ReconcileOperation => reconcile_with_same_operation(),
        RetryAdvice::Never => return Err(error),
    },
}
```

The `ErrorCode` enum is non-exhaustive. Common categories include:

| Category | Codes to expect |
| --- | --- |
| Input and limits | `InvalidRequest`, `InvalidKey`, `InvalidBranch`, `InvalidLimit`, `InvalidRange`, `EntityTooLarge`, `UnsupportedParameter` |
| Repository setup | `RepositoryNotInitialized`, `RepositoryFormatConflict`, `UnsupportedRepositoryFormat`, `ProviderNotQualified`, `MissingCapability`, `InvalidBucket` |
| Missing data | `NoSuchKey`, `NoSuchVersion`, `NoSuchBranch`, `NoSuchBatch`, `NoSuchUpload`, `MissingClosure` |
| Concurrent state | `PreconditionFailed`, `RefConflict`, `BatchConflict`, `IdempotencyConflict`, `UploadConflict`, `NotModified` |
| Maintenance and history | `BatchExpired`, `HistoryLimitExceeded`, `NoMergeBase`, `AmbiguousMergeBase`, `MergeConflict`, `InvalidContinuationToken` |
| Integrity | `ChecksumMismatch`, `CorruptNode`, `CorruptContent`, `CorruptCommit` |
| Provider and runtime | `PermissionDenied`, `Throttled`, `Timeout`, `OperationCanceled`, `OutcomeUnknown`, `Transport`, `InternalInvariant` |

Use `ReloadHead` when a branch CAS or expected-head check fails. Use `ReconcileOperation` or the `operation_id` on an ambiguous write. Never create a new operation ID for a request whose publication outcome is unknown.

Read-only clients reject publication with `PreconditionFailed`. Detached checkouts reject branch-ref and mutation calls with `InvalidRevision`. A provider capability or attestation failure returns `ProviderNotQualified` or `MissingCapability` before the client publishes.

## Respect canonical limits

The repository stores its canonical limits in the create-once format marker. The default values are:

| Limit | Default |
| --- | ---: |
| Logical key length | 1,024 bytes |
| List page size | 1,000 entries |
| Multi-delete size | 1,000 keys |
| Mutations per commit | 10,000 logical keys |
| Repository object size | 5 TiB |

The AWS qualification profile reports a 5 TiB provider object limit and a 5 GiB single-PUT limit. The built-in SILO upload paths use one `PutObject`, so a body must fit the provider’s single-PUT limit. Use the external whole-object handoff for larger or provider-managed resumable transfers, subject to the provider’s complete-object limit.

List, diff, merge, fsck, restore, repair, history-transfer, and GC methods accept page or step limits so workers can bound memory and provider request time. Persist the returned cursor after each page. A cursor is repository- and job-bound; do not reuse it across repositories, branches, snapshots, or jobs.

Keep a GC grace period longer than every unpublished operation. Keep provider versioning enabled for the repository lifetime. Do not change the stored state-tree format, canonical limits, provider profile, or idempotency retention in place.

The client’s architecture supports large repositories, but production readiness depends on provider quotas, key distribution, concurrency, cache size, throttling, latency, cost, and maintenance timing. Use [`QUALIFICATION.md`](QUALIFICATION.md) and [`PERFORMANCE-ENVELOPE-2026-08-13.md`](PERFORMANCE-ENVELOPE-2026-08-13.md) for workload-specific gates. RustFS conformance demonstrates protocol behavior, not Amazon S3 production economics.

## Run the complete examples

Start the pinned local RustFS service, then run the compiled scenarios:

```bash
docker compose -f docker-compose.rustfs.yml up -d
scripts/run_rustfs_examples.sh
```

The examples use isolated prefixes by default. Run one scenario directly:

```bash
cargo run --locked --manifest-path Cargo.toml \
    -p silo-s3-client --example basic_object_workflow
```

The repository’s CI-compilable examples cover the end-to-end workflows in this reference:

| Scenario | Source |
| --- | --- |
| CRUD, metadata, ranges, copy, listing, and history | [`basic_object_workflow.rs`](client/examples/basic_object_workflow.rs) |
| Durable atomic batches and streamed input | [`atomic_batch_and_streaming.rs`](client/examples/atomic_batch_and_streaming.rs) |
| Branch isolation, bounded diff, merge, log, and reflog | [`branch_diff_merge.rs`](client/examples/branch_diff_merge.rs) |
| Historical restore, reset, and reflog recovery | [`restore_and_recovery.rs`](client/examples/restore_and_recovery.rs) |
| Commit-DAG transfer and backup verification | [`history_transfer_and_backup.rs`](client/examples/history_transfer_and_backup.rs) |
| Deep fsck, cache prewarm, metrics, retention, and GC | [`integrity_gc_and_observability.rs`](client/examples/integrity_gc_and_observability.rs) |

For operational runbooks, read [`OPERATIONS.md`](OPERATIONS.md). For the application-level walkthrough, read [`client/README.md`](client/README.md). For the persisted format and design rationale, read [`SILO-DESIGN.md`](SILO-DESIGN.md) and [`docs/adr`](docs/adr).

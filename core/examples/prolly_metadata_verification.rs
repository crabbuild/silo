//! Metadata-only Prolly verification for a million tiny logical files.
//!
//! This intentionally does not create payload objects.  It drives the same
//! async Prolly engine and packed node store used by repository commit
//! sessions, but leaves node publication in the session's in-memory pending
//! set.  That makes the result a tree/metadata measurement rather than a
//! durability or provider-latency measurement.

use std::{
    collections::BTreeMap,
    env,
    sync::Arc,
    time::{Duration, Instant},
};

use prolly::{AsyncProlly, Config, Mutation, RuntimeConfig, Tree, TreeFormat};
use silo_s3_core::{
    encode_canonical, Checksums, CommitGeneration, CurrentObject, LogicalObjectVersionBody,
    LogicalObjectVersionKind, MemoryObjectPlane, ObjectHeaders, ObjectPath, ObjectVersion,
    ObjectVersionOrder, OperationId, PayloadBinding, ProllyObjectStore, RepositoryId,
};

type BenchResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const DEFAULT_FILES: usize = 1_000_000;
const DEFAULT_BATCH: usize = 10_000;
const DEFAULT_VALUE_BYTES: usize = 20;
const DEFAULT_SAMPLES: usize = 10_000;

#[derive(Default)]
struct Totals {
    batches: usize,
    input_mutations: usize,
    effective_mutations: usize,
    entries_streamed: usize,
    nodes_read: usize,
    nodes_written: usize,
    nodes_reused: usize,
    bytes_read: usize,
    bytes_written: usize,
    parallel_width: usize,
    parallel_tasks: usize,
    structural_islands: usize,
    coalesced_islands: usize,
    key_stable_fast_path_batches: usize,
    batched_value_update_path_batches: usize,
}

impl Totals {
    fn add(&mut self, stats: &prolly::WriteStats) {
        self.batches += 1;
        self.input_mutations += stats.input_mutations as usize;
        self.effective_mutations += stats.effective_mutations as usize;
        self.entries_streamed += stats.entries_streamed as usize;
        self.nodes_read += stats.nodes_read as usize;
        self.nodes_written += stats.nodes_written as usize;
        self.nodes_reused += stats.nodes_reused as usize;
        self.bytes_read += stats.bytes_read as usize;
        self.bytes_written += stats.bytes_written as usize;
        self.parallel_width = self.parallel_width.max(stats.parallel_width as usize);
        self.parallel_tasks += stats.parallel_tasks as usize;
        self.structural_islands += stats.structural_islands as usize;
        self.coalesced_islands += stats.coalesced_islands as usize;
        self.key_stable_fast_path_batches += usize::from(stats.used_key_stable_fast_path);
        self.batched_value_update_path_batches += usize::from(stats.used_batched_value_update_path);
    }
}

fn env_usize(name: &str, default: usize) -> BenchResult<usize> {
    Ok(env::var(name)
        .unwrap_or_else(|_| default.to_string())
        .parse::<usize>()?)
}

fn key(index: usize) -> Vec<u8> {
    format!("repo/files/{index:07}.txt").into_bytes()
}

fn value(index: usize, value_bytes: usize) -> Vec<u8> {
    let mut value = vec![0_u8; value_bytes];
    let encoded = (index as u64).to_be_bytes();
    let copy_len = encoded.len().min(value.len());
    value[..copy_len].copy_from_slice(&encoded[..copy_len]);
    for (offset, byte) in value.iter_mut().enumerate().skip(copy_len) {
        *byte = (index as u8).wrapping_add(offset as u8);
    }
    value
}

fn config(target_entries: usize) -> BenchResult<Config> {
    let target = u64::try_from(target_entries)?;
    let mut format = TreeFormat::default();
    format.chunking.target = target;
    format.chunking.max = target.saturating_mul(8);
    format.chunking.rule = prolly::BoundaryRule::HashThreshold {
        factor: u32::try_from(target_entries)?,
    };
    format
        .validate()
        .map_err(|error| format!("invalid tree format: {error}"))?;
    Ok(Config {
        format,
        runtime: RuntimeConfig {
            // Keep this verification focused on tree work. The packed session
            // itself is already the node working set, so an unbounded engine
            // cache avoids evict/reload noise during the write phase.
            node_cache_max_nodes: None,
            node_cache_max_bytes: None,
            read_parallelism: 32,
        },
    })
}

fn root_string(tree: &Tree) -> String {
    tree.root
        .as_ref()
        .map(|root| format!("{root:?}"))
        .unwrap_or_else(|| "empty".to_string())
}

async fn verify(
    files: usize,
    batch_size: usize,
    value_bytes: usize,
    target_entries: usize,
    samples: usize,
) -> BenchResult {
    let config = config(target_entries)?;
    let plane = Arc::new(MemoryObjectPlane::new(false));
    let store = ProllyObjectStore::new_packed(plane.clone(), "metadata-only-verification");
    let engine = AsyncProlly::new(store, config);
    let mut tree = engine.create();
    let mut totals = Totals::default();
    let started = Instant::now();

    for start in (0..files).step_by(batch_size) {
        let end = (start + batch_size).min(files);
        let mutations = (start..end)
            .map(|index| Mutation::Upsert {
                key: key(index),
                val: value(index, value_bytes),
            })
            .collect();
        let (next_tree, stats) = engine.batch_with_write_stats(&tree, mutations).await?;
        totals.add(&stats);
        tree = next_tree;

        if end == files || end.is_multiple_of(100_000) {
            let elapsed = started.elapsed().as_secs_f64();
            println!(
                "PROGRESS files={end} wall_ms={:.3} files_per_second={:.2} nodes_written={} metadata_bytes_written={}",
                elapsed * 1_000.0,
                end as f64 / elapsed,
                totals.nodes_written,
                totals.bytes_written,
            );
        }
    }

    let write_elapsed = started.elapsed();
    let stats_started = Instant::now();
    let tree_stats = engine.collect_stats(&tree).await?;
    let stats_elapsed = stats_started.elapsed();

    let sample_count = samples.min(files);
    let sample_keys = (0..sample_count)
        .map(|sample| key(sample * files / sample_count.max(1)))
        .collect::<Vec<_>>();
    let read_started = Instant::now();
    let values = engine.get_many(&tree, &sample_keys).await?;
    let read_elapsed = read_started.elapsed();
    if values.len() != sample_count
        || values.iter().enumerate().any(|(offset, result)| {
            let index = offset * files / sample_count.max(1);
            result.as_deref() != Some(value(index, value_bytes).as_slice())
        })
    {
        return Err("metadata point-read verification failed".into());
    }

    let scan_started = Instant::now();
    let mut scanned_bytes = 0_usize;
    let scanned = engine
        .scan_range(&tree, b"repo/files/", None, |entry| {
            scanned_bytes += entry.key().len() + entry.value().len();
        })
        .await?;
    let scan_elapsed = scan_started.elapsed();
    if scanned != files as u64 {
        return Err(format!("metadata scan found {scanned} entries, expected {files}").into());
    }

    let requests = plane.request_snapshot();
    println!(
        "CONFIG files={files} batch_size={batch_size} value_bytes={value_bytes} target_entries={target_entries} logical_file_bytes={} samples={sample_count} packed_pending=true",
        files.saturating_mul(value_bytes),
    );
    println!(
        "WRITE wall_ms={:.3} files_per_second={:.2} batches={} input_mutations={} effective_mutations={} entries_streamed={} nodes_read={} nodes_written={} nodes_reused={} bytes_read={} bytes_written={} parallel_width={} parallel_tasks={} structural_islands={} coalesced_islands={} key_stable_fast_path_batches={} batched_value_update_path_batches={}",
        write_elapsed.as_secs_f64() * 1_000.0,
        files as f64 / write_elapsed.as_secs_f64(),
        totals.batches,
        totals.input_mutations,
        totals.effective_mutations,
        totals.entries_streamed,
        totals.nodes_read,
        totals.nodes_written,
        totals.nodes_reused,
        totals.bytes_read,
        totals.bytes_written,
        totals.parallel_width,
        totals.parallel_tasks,
        totals.structural_islands,
        totals.coalesced_islands,
        totals.key_stable_fast_path_batches,
        totals.batched_value_update_path_batches,
    );
    println!(
        "TREE root={} nodes={} leaves={} internal_nodes={} height={} tree_bytes={} avg_node_bytes={:.2} entries={} avg_entries_per_node={:.2} key_bytes={} value_bytes={} stats_wall_ms={:.3}",
        root_string(&tree),
        tree_stats.num_nodes,
        tree_stats.num_leaves,
        tree_stats.num_internal_nodes,
        tree_stats.tree_height,
        tree_stats.total_tree_size_bytes,
        tree_stats.avg_node_size_bytes,
        tree_stats.total_key_value_pairs,
        tree_stats.avg_entries_per_node,
        tree_stats.total_keys_size_bytes,
        tree_stats.total_values_size_bytes,
        stats_elapsed.as_secs_f64() * 1_000.0,
    );
    println!(
        "READ samples={sample_count} wall_ms={:.3} reads_per_second={:.2} scan_entries={scanned} scan_bytes={scanned_bytes} scan_wall_ms={:.3}",
        read_elapsed.as_secs_f64() * 1_000.0,
        sample_count as f64 / read_elapsed.as_secs_f64().max(f64::MIN_POSITIVE),
        scan_elapsed.as_secs_f64() * 1_000.0,
    );
    println!(
        "OBJECT_PLANE get={} head={} immutable_put={} immutable_transfer={} compare_exchange={} list={} delete_exact={} total={} payload_puts=0 payload_bytes=0",
        requests.get,
        requests.head,
        requests.immutable_put,
        requests.immutable_transfer,
        requests.compare_exchange,
        requests.list,
        requests.delete_exact,
        requests.total(),
    );
    Ok(())
}

struct StateEntry {
    version_key: Vec<u8>,
    current_bytes: Vec<u8>,
    version_bytes: Vec<u8>,
}

fn version_tree_key(
    key: &[u8],
    order: ObjectVersionOrder,
    version: silo_s3_core::ObjectVersionId,
) -> Vec<u8> {
    let mut output = Vec::with_capacity(key.len() + 2 + 8 + 4 + 32);
    for byte in key {
        if *byte == 0 {
            output.extend_from_slice(&[0, 0xff]);
        } else {
            output.push(*byte);
        }
    }
    output.extend_from_slice(&[0, 0]);
    output.extend(order.commit_generation.0.to_be_bytes().map(|byte| !byte));
    output.extend(order.mutation_ordinal.to_be_bytes().map(|byte| !byte));
    output.extend(version.as_bytes().iter().map(|byte| !byte));
    output
}

fn state_entry(
    index: usize,
    batch_size: usize,
    repository: RepositoryId,
    binding: &PayloadBinding,
) -> BenchResult<StateEntry> {
    let key = key(index);
    let order = ObjectVersionOrder {
        commit_generation: CommitGeneration(u64::try_from(index / batch_size + 1)?),
        mutation_ordinal: u32::try_from(index % batch_size)?,
    };
    let version = ObjectVersion::derive(
        repository,
        &key,
        OperationId::nil(),
        LogicalObjectVersionBody {
            order,
            created_at_millis: order.commit_generation.0,
            kind: LogicalObjectVersionKind::Live {
                size: 20,
                logical_etag: "\"synthetic-20-byte-file\"".to_string(),
                headers: ObjectHeaders::default(),
                checksums: Checksums {
                    sha256: Some(binding.checksum_sha256),
                    ..Checksums::default()
                },
                user_metadata: BTreeMap::new(),
                tags: BTreeMap::new(),
            },
        },
        Some(binding.clone()),
    )?;
    Ok(StateEntry {
        version_key: version_tree_key(&key, order, version.id),
        current_bytes: encode_canonical(&CurrentObject {
            version: version.clone(),
        })?,
        version_bytes: encode_canonical(&version)?,
    })
}

async fn verify_repository_state(
    files: usize,
    batch_size: usize,
    target_entries: usize,
    samples: usize,
) -> BenchResult {
    let config = config(target_entries)?;
    let plane = Arc::new(MemoryObjectPlane::new(false));
    let store = ProllyObjectStore::new_packed(plane.clone(), "metadata-state-verification");
    let engine = AsyncProlly::new(store, config);
    let mut objects = engine.create();
    let mut versions = engine.create();
    let repository = RepositoryId::from_hash([0x37; 32]);
    let binding = PayloadBinding {
        path: ObjectPath::new("payloads/sha256/synthetic-20-byte-file")?,
        provider_version_id: None,
        provider_etag: "synthetic-20-byte-file".to_string(),
        checksum_sha256: [0x43; 32],
    };
    let mut object_totals = Totals::default();
    let mut version_totals = Totals::default();
    let mut metadata_build_elapsed = Duration::ZERO;
    let mut object_apply_elapsed = Duration::ZERO;
    let mut version_apply_elapsed = Duration::ZERO;
    let started = Instant::now();

    for start in (0..files).step_by(batch_size) {
        let end = (start + batch_size).min(files);
        let metadata_build_started = Instant::now();
        let mut object_mutations = Vec::with_capacity(end - start);
        let mut version_mutations = Vec::with_capacity(end - start);
        for index in start..end {
            let entry = state_entry(index, batch_size, repository, &binding)?;
            object_mutations.push(Mutation::Upsert {
                key: key(index),
                val: entry.current_bytes,
            });
            version_mutations.push(Mutation::Upsert {
                key: entry.version_key,
                val: entry.version_bytes,
            });
        }
        metadata_build_elapsed += metadata_build_started.elapsed();
        let object_apply_started = Instant::now();
        let (next_objects, object_stats) = engine
            .batch_with_write_stats(&objects, object_mutations)
            .await?;
        object_apply_elapsed += object_apply_started.elapsed();
        let version_apply_started = Instant::now();
        let (next_versions, version_stats) = engine
            .batch_with_write_stats(&versions, version_mutations)
            .await?;
        version_apply_elapsed += version_apply_started.elapsed();
        object_totals.add(&object_stats);
        version_totals.add(&version_stats);
        objects = next_objects;
        versions = next_versions;

        if end == files || end.is_multiple_of(100_000) {
            let elapsed = started.elapsed().as_secs_f64();
            println!(
                "STATE_PROGRESS files={end} wall_ms={:.3} files_per_second={:.2} object_nodes_written={} version_nodes_written={}",
                elapsed * 1_000.0,
                end as f64 / elapsed,
                object_totals.nodes_written,
                version_totals.nodes_written,
            );
        }
    }

    let write_elapsed = started.elapsed();
    let object_stats_started = Instant::now();
    let object_tree_stats = engine.collect_stats(&objects).await?;
    let object_stats_elapsed = object_stats_started.elapsed();
    let version_stats_started = Instant::now();
    let version_tree_stats = engine.collect_stats(&versions).await?;
    let version_stats_elapsed = version_stats_started.elapsed();

    let sample_count = samples.min(files);
    let sample_indexes = (0..sample_count)
        .map(|sample| sample * files / sample_count.max(1))
        .collect::<Vec<_>>();
    let sample_keys = sample_indexes
        .iter()
        .map(|index| key(*index))
        .collect::<Vec<_>>();
    let read_started = Instant::now();
    let values = engine.get_many(&objects, &sample_keys).await?;
    let read_elapsed = read_started.elapsed();
    if values.len() != sample_count {
        return Err("repository-state point-read count mismatch".into());
    }
    for (index, actual) in sample_indexes.iter().zip(values) {
        let expected = state_entry(*index, batch_size, repository, &binding)?.current_bytes;
        if actual.as_deref() != Some(expected.as_slice()) {
            return Err(format!("repository-state point-read failed for index {index}").into());
        }
    }

    let scan_started = Instant::now();
    let mut scanned_bytes = 0_usize;
    let object_scanned = engine
        .scan_range(&objects, b"repo/files/", None, |entry| {
            scanned_bytes += entry.key().len() + entry.value().len();
        })
        .await?;
    let version_scanned = engine
        .scan_range(&versions, b"repo/files/", None, |entry| {
            scanned_bytes += entry.key().len() + entry.value().len();
        })
        .await?;
    let scan_elapsed = scan_started.elapsed();
    if object_scanned != files as u64 || version_scanned != files as u64 {
        return Err(format!(
            "repository-state scan mismatch: objects={object_scanned}, versions={version_scanned}, expected={files}"
        )
        .into());
    }

    let requests = plane.request_snapshot();
    println!(
        "STATE_CONFIG files={files} batch_size={batch_size} file_bytes=20 logical_file_bytes={} target_entries={target_entries} samples={sample_count} packed_pending=true",
        files.saturating_mul(20),
    );
    println!(
        "STATE_WRITE wall_ms={:.3} files_per_second={:.2} metadata_build_ms={:.3} object_apply_ms={:.3} version_apply_ms={:.3} object_batches={} object_nodes_written={} object_bytes_written={} object_key_stable_fast_path_batches={} version_batches={} version_nodes_written={} version_bytes_written={} version_key_stable_fast_path_batches={}",
        write_elapsed.as_secs_f64() * 1_000.0,
        files as f64 / write_elapsed.as_secs_f64(),
        metadata_build_elapsed.as_secs_f64() * 1_000.0,
        object_apply_elapsed.as_secs_f64() * 1_000.0,
        version_apply_elapsed.as_secs_f64() * 1_000.0,
        object_totals.batches,
        object_totals.nodes_written,
        object_totals.bytes_written,
        object_totals.key_stable_fast_path_batches,
        version_totals.batches,
        version_totals.nodes_written,
        version_totals.bytes_written,
        version_totals.key_stable_fast_path_batches,
    );
    println!(
        "OBJECT_TREE root={} nodes={} leaves={} internal_nodes={} height={} tree_bytes={} entries={} key_bytes={} value_bytes={} stats_wall_ms={:.3}",
        root_string(&objects),
        object_tree_stats.num_nodes,
        object_tree_stats.num_leaves,
        object_tree_stats.num_internal_nodes,
        object_tree_stats.tree_height,
        object_tree_stats.total_tree_size_bytes,
        object_tree_stats.total_key_value_pairs,
        object_tree_stats.total_keys_size_bytes,
        object_tree_stats.total_values_size_bytes,
        object_stats_elapsed.as_secs_f64() * 1_000.0,
    );
    println!(
        "VERSION_TREE root={} nodes={} leaves={} internal_nodes={} height={} tree_bytes={} entries={} key_bytes={} value_bytes={} stats_wall_ms={:.3}",
        root_string(&versions),
        version_tree_stats.num_nodes,
        version_tree_stats.num_leaves,
        version_tree_stats.num_internal_nodes,
        version_tree_stats.tree_height,
        version_tree_stats.total_tree_size_bytes,
        version_tree_stats.total_key_value_pairs,
        version_tree_stats.total_keys_size_bytes,
        version_tree_stats.total_values_size_bytes,
        version_stats_elapsed.as_secs_f64() * 1_000.0,
    );
    println!(
        "STATE_READ samples={sample_count} wall_ms={:.3} reads_per_second={:.2} object_scan_entries={object_scanned} version_scan_entries={version_scanned} combined_scan_bytes={scanned_bytes} scan_wall_ms={:.3}",
        read_elapsed.as_secs_f64() * 1_000.0,
        sample_count as f64 / read_elapsed.as_secs_f64().max(f64::MIN_POSITIVE),
        scan_elapsed.as_secs_f64() * 1_000.0,
    );
    println!(
        "OBJECT_PLANE get={} head={} immutable_put={} immutable_transfer={} compare_exchange={} list={} delete_exact={} total={} payload_puts=0 payload_bytes=0",
        requests.get,
        requests.head,
        requests.immutable_put,
        requests.immutable_transfer,
        requests.compare_exchange,
        requests.list,
        requests.delete_exact,
        requests.total(),
    );
    Ok(())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() -> BenchResult {
    let files = env_usize("SILO_METADATA_FILES", DEFAULT_FILES)?;
    let batch_size = env_usize("SILO_METADATA_BATCH", DEFAULT_BATCH)?;
    let value_bytes = env_usize("SILO_METADATA_VALUE_BYTES", DEFAULT_VALUE_BYTES)?;
    let target_entries = env_usize("SILO_METADATA_TARGET_ENTRIES", 128)?;
    let samples = env_usize("SILO_METADATA_SAMPLES", DEFAULT_SAMPLES)?;
    let mode = env::var("SILO_METADATA_MODE").unwrap_or_else(|_| "state".to_string());
    if files == 0 || batch_size == 0 || value_bytes == 0 || target_entries < 4 {
        return Err(
            "SILO_METADATA_FILES, SILO_METADATA_BATCH, SILO_METADATA_VALUE_BYTES, and SILO_METADATA_TARGET_ENTRIES must be positive (target >= 4)"
                .into(),
        );
    }
    match mode.as_str() {
        "minimal" => verify(files, batch_size, value_bytes, target_entries, samples).await,
        "state" => verify_repository_state(files, batch_size, target_entries, samples).await,
        "both" => {
            verify(files, batch_size, value_bytes, target_entries, samples).await?;
            verify_repository_state(files, batch_size, target_entries, samples).await
        }
        _ => Err("SILO_METADATA_MODE must be minimal, state, or both".into()),
    }
}

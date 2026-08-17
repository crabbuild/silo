# silo-s3-core

The AWS-independent core of [SILO](https://github.com/crabuild/silo), an
immutable version-control ledger layered over S3-compatible object storage.

This crate owns the durable repository model, Prolly-tree state, commits,
branches, tags, journals, recovery, fsck, merge planning, and garbage
collection. The provider adapter lives in `silo-s3-client`.

The persisted format retains the `prolly-s3` compatibility domain so existing
repositories remain readable across the SILO extraction.

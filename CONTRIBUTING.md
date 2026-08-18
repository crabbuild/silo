# Contributing to SILO

SILO is an open-source CrabBuild repository. Contributions are reviewed
through pull requests and must preserve the durable repository contract. SILO
was extracted from the [Prolly repository](https://github.com/crabbuild/prolly).

## Before opening a pull request

```bash
cargo fmt --all -- --check
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
python3 spec/prolly-s3/conformance/verify.py
scripts/check_clean_downstream.sh
```

RustFS integration is opt-in and requires an isolated local bucket:

```bash
docker compose -f docker-compose.rustfs.yml up -d
SILO_S3_RUSTFS=1 cargo test --workspace -p silo-s3-client \
  --test rustfs_repository -- --nocapture
```

## Change rules

- Do not change canonical encoding, domain-separated IDs, or durable paths
  without a versioned compatibility decision and golden fixtures.
- Do not pack user payloads, split them into repository-managed chunks, or
  persist provider multipart state.
- Keep provider-independent behavior in `silo-s3-core` and AWS SDK behavior in
  `silo-s3-client`.
- Add deterministic core tests for correctness changes and provider tests for
  S3/RustFS behavior.
- Do not include credentials, generated benchmark data, or provider state.

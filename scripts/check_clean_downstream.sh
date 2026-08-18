#!/usr/bin/env bash
set -euo pipefail

consumer_checks_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../consumer-checks" && pwd)"
rust_toolchain="${SILO_RUST_TOOLCHAIN:-1.94.1}"
if [[ -n "${SILO_S3_CARGO_TARGET_DIR:-}" ]]; then
  target_root="$SILO_S3_CARGO_TARGET_DIR/clean-downstream"
  mkdir -p "$target_root/rust-$rust_toolchain"
else
  target_root="$(mktemp -d "${TMPDIR:-/tmp}/silo-downstream.XXXXXX")"
  trap 'rm -rf -- "$target_root"' EXIT
fi

export RUSTC_WRAPPER=""

CARGO_TARGET_DIR="$target_root/rust-$rust_toolchain" \
cargo +"$rust_toolchain" check \
  --manifest-path "$consumer_checks_root/Cargo.toml" \
  -p silo-s3-downstream-core-check --locked
CARGO_TARGET_DIR="$target_root/rust-$rust_toolchain" \
cargo +"$rust_toolchain" check \
  --manifest-path "$consumer_checks_root/Cargo.toml" \
  -p silo-s3-downstream-client-check \
  --no-default-features --locked
CARGO_TARGET_DIR="$target_root/rust-$rust_toolchain" \
cargo +"$rust_toolchain" check \
  --manifest-path "$consumer_checks_root/Cargo.toml" \
  -p silo-s3-downstream-client-check \
  --no-default-features --features foyer-cache --locked

echo "CLEAN_DOWNSTREAM_COMPLETE rust=${rust_toolchain} feature_sets=minimal,foyer-cache"

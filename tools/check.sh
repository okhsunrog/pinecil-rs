#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all --check
cargo build --release --locked
cargo clippy --release --locked -- -D warnings
uv run --no-project tools/audit_elf.py
uv run --no-project tools/test_flash_image.py target/riscv32imc-unknown-none-elf/release/pinecil-rs

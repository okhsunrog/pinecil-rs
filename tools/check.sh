#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all --check
# Cargo discovers config from cwd, not --manifest-path. Run host-only tests
# outside the embedded config so they use the pinned toolchain's prebuilt std.
project_dir=$PWD
toolchain_root=$(rustc --print sysroot)
host_target=$(rustc -vV | awk '/^host:/ {print $2}')
(cd ..; RUSTC="$toolchain_root/bin/rustc" "$toolchain_root/bin/cargo" test --locked --manifest-path "$project_dir/Cargo.toml" --lib --target "$host_target")
cargo build --release --locked
cargo clippy --release --locked --bin pinecil-rs --example hardware-check -- -D warnings
cargo build --release --locked --example hardware-check
uv run --no-project tools/audit_elf.py
uv run --no-project tools/audit_elf.py target/riscv32imfc-unknown-none-elf/release/examples/hardware-check
uv run --no-project tools/test_flash_image.py target/riscv32imfc-unknown-none-elf/release/pinecil-rs

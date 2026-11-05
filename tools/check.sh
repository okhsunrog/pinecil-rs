#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
uv run --no-project tools/check_sources.py
cargo fmt --all --check
cargo clippy --release --locked --features pd-diagnostics --bin pinecil-rs -- -D warnings
cargo build --release --locked --features pd-manual-start
uv run --no-project tools/audit_elf.py
# Cargo discovers config from cwd, not --manifest-path. Run host-only tests
# outside the embedded config so they use the pinned toolchain's prebuilt std.
project_dir=$PWD
toolchain_root=$(rustc --print sysroot)
host_target=$(rustc -vV | awk '/^host:/ {print $2}')
(cd ..; RUSTC="$toolchain_root/bin/rustc" "$toolchain_root/bin/cargo" test --locked --manifest-path "$project_dir/Cargo.toml" --lib --target "$host_target")
cargo clippy --release --locked --no-default-features --features fpu --bin pinecil-rs -- -D warnings
cargo build --release --locked --no-default-features --features fpu
uv run --no-project tools/audit_elf.py
cargo build --release --locked
cargo clippy --release --locked --bin pinecil-rs --example hardware-check -- -D warnings
cargo build --release --locked --example hardware-check
uv run --no-project tools/audit_elf.py
uv run --no-project tools/audit_elf.py target/riscv32imfc-unknown-none-elf/release/examples/hardware-check
uv run --no-project tools/test_flash_image.py target/riscv32imfc-unknown-none-elf/release/pinecil-rs

BLE_CODE_PLACEMENT=itcm cargo clippy --release --locked --features ble-diagnostics --example ble -- -D warnings
BLE_CODE_PLACEMENT=itcm cargo build --release --locked --features ble-diagnostics --example ble
uv run --no-project tools/audit_elf.py target/riscv32imfc-unknown-none-elf/release/examples/ble
BLE_CODE_PLACEMENT=xip cargo build --release --locked --features ble-diagnostics --example ble
uv run --no-project tools/audit_elf.py target/riscv32imfc-unknown-none-elf/release/examples/ble --ble-placement=xip

cargo clippy --release --locked --features ble --example ble -- -D warnings
cargo build --release --locked --features ble --example ble
uv run --no-project tools/audit_elf.py target/riscv32imfc-unknown-none-elf/release/examples/ble
uv run --no-project tools/test_flash_image.py target/riscv32imfc-unknown-none-elf/release/examples/ble

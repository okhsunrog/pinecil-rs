use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=pinecil.x");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let source = if env::var_os("CARGO_FEATURE_BLE").is_some() {
        "pinecil-ble.x"
    } else {
        "pinecil.x"
    };
    println!("cargo:rerun-if-changed=pinecil-ble.x");
    fs::copy(source, out.join("pinecil.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    if env::var("CARGO_CFG_TARGET_ARCH").unwrap() == "riscv32" {
        println!("cargo:rustc-link-arg=-T{}", out.join("pinecil.x").display());
    }
}

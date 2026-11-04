use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=pinecil.x");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::copy("pinecil.x", out.join("pinecil.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
}

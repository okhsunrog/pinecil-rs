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
    println!("cargo:rerun-if-env-changed=BLE_CODE_PLACEMENT");
    let mut script = fs::read_to_string(source).unwrap();
    if env::var_os("CARGO_FEATURE_BLE_DIAGNOSTICS").is_some() {
        println!("cargo:rustc-link-arg=--undefined=ble_trace_timer_store");
        // Keep data/BSS/stack at identical addresses in both placement variants.
        let hot = "*(.text.rwble_* .text.rwip_* .text.lld_* .text.ea_*)\n        *(.text.ble_co_list_* .text.ble_ke_event_* .bleromro .bleromro.*)";
        assert_eq!(
            script.matches(hot).count(),
            1,
            "hot-code linker pattern changed"
        );
        let placement = env::var("BLE_CODE_PLACEMENT").unwrap_or_else(|_| "itcm".into());
        assert!(matches!(placement.as_str(), "itcm" | "xip"));
        if placement == "xip" {
            script = script.replace(hot, "");
        }
        script = script.replace("KEEP(*(.init))", "KEEP(*(.init)) *(.text.ble_trace_fast)");
        script = script.replace(".data : AT(", ".data ORIGIN(ITCM) + 0x7800 : AT(");
        script = script.replace(
            "    /DISCARD/",
            "    .ble_trace (NOLOAD) : ALIGN(64) { KEEP(*(.ble_trace)) } > STACK\n    /DISCARD/",
        );
        script.push_str("\nASSERT(ADDR(.ble_trace) + SIZEOF(.ble_trace) <= 0x22024000, \"BLE trace overlaps RF scratch\")\n");
        for function in [
            "rwble_isr",
            "rwip_schedule",
            "ea_finetimer_isr",
            "ea_sw_isr",
        ] {
            println!("cargo:rustc-link-arg=--wrap={function}");
        }
    }
    fs::write(out.join("pinecil.x"), script).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    if env::var("CARGO_CFG_TARGET_ARCH").unwrap() == "riscv32" {
        println!("cargo:rustc-link-arg=-T{}", out.join("pinecil.x").display());
    }
}

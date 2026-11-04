//! Runtime f32 arithmetic and all-register IRQ-context test.
use core::arch::asm;

pub fn arithmetic() {
    let x = core::hint::black_box(1.5f32);
    let y = core::hint::black_box(2.0f32);
    assert_eq!((x * y + core::hint::black_box(0.25)).to_bits(), 0x4050_0000);
    assert_eq!((core::hint::black_box(6.0f32) / y).to_bits(), 0x4040_0000);
    let root: f32;
    unsafe {
        asm!("fsqrt.s {out}, {input}", out = out(freg) root, input = in(freg) core::hint::black_box(9.0f32));
    }
    assert_eq!(root.to_bits(), 0x4040_0000);
}

/// Rust sees every FPR as clobbered, so its ABI-preserved registers are saved
/// around this block. The distinct bit patterns expose swapped/missing spills.
#[inline(never)]
pub fn registers() -> [u32; 33] {
    let mut state = [0u32; 33];
    unsafe {
        asm!(
            "li t0, 0x61", "fscsr t0",
            "li t0, 0x3f800000", "fmv.w.x f0, t0",
            "li t0, 0x3f800001", "fmv.w.x f1, t0",
            "li t0, 0x3f800002", "fmv.w.x f2, t0",
            "li t0, 0x3f800003", "fmv.w.x f3, t0",
            "li t0, 0x3f800004", "fmv.w.x f4, t0",
            "li t0, 0x3f800005", "fmv.w.x f5, t0",
            "li t0, 0x3f800006", "fmv.w.x f6, t0",
            "li t0, 0x3f800007", "fmv.w.x f7, t0",
            "li t0, 0x3f800008", "fmv.w.x f8, t0",
            "li t0, 0x3f800009", "fmv.w.x f9, t0",
            "li t0, 0x3f80000a", "fmv.w.x f10, t0",
            "li t0, 0x3f80000b", "fmv.w.x f11, t0",
            "li t0, 0x3f80000c", "fmv.w.x f12, t0",
            "li t0, 0x3f80000d", "fmv.w.x f13, t0",
            "li t0, 0x3f80000e", "fmv.w.x f14, t0",
            "li t0, 0x3f80000f", "fmv.w.x f15, t0",
            "li t0, 0x3f800010", "fmv.w.x f16, t0",
            "li t0, 0x3f800011", "fmv.w.x f17, t0",
            "li t0, 0x3f800012", "fmv.w.x f18, t0",
            "li t0, 0x3f800013", "fmv.w.x f19, t0",
            "li t0, 0x3f800014", "fmv.w.x f20, t0",
            "li t0, 0x3f800015", "fmv.w.x f21, t0",
            "li t0, 0x3f800016", "fmv.w.x f22, t0",
            "li t0, 0x3f800017", "fmv.w.x f23, t0",
            "li t0, 0x3f800018", "fmv.w.x f24, t0",
            "li t0, 0x3f800019", "fmv.w.x f25, t0",
            "li t0, 0x3f80001a", "fmv.w.x f26, t0",
            "li t0, 0x3f80001b", "fmv.w.x f27, t0",
            "li t0, 0x3f80001c", "fmv.w.x f28, t0",
            "li t0, 0x3f80001d", "fmv.w.x f29, t0",
            "li t0, 0x3f80001e", "fmv.w.x f30, t0",
            "li t0, 0x3f80001f", "fmv.w.x f31, t0",
            ".global bl706_fpu_test_armed",
            ".set bl706_fpu_test_armed, .",
            "2: addi t2, t2, -1", "bnez t2, 2b",
            "fsw f0, 0({out})",
            "fsw f1, 4({out})",
            "fsw f2, 8({out})",
            "fsw f3, 12({out})",
            "fsw f4, 16({out})",
            "fsw f5, 20({out})",
            "fsw f6, 24({out})",
            "fsw f7, 28({out})",
            "fsw f8, 32({out})",
            "fsw f9, 36({out})",
            "fsw f10, 40({out})",
            "fsw f11, 44({out})",
            "fsw f12, 48({out})",
            "fsw f13, 52({out})",
            "fsw f14, 56({out})",
            "fsw f15, 60({out})",
            "fsw f16, 64({out})",
            "fsw f17, 68({out})",
            "fsw f18, 72({out})",
            "fsw f19, 76({out})",
            "fsw f20, 80({out})",
            "fsw f21, 84({out})",
            "fsw f22, 88({out})",
            "fsw f23, 92({out})",
            "fsw f24, 96({out})",
            "fsw f25, 100({out})",
            "fsw f26, 104({out})",
            "fsw f27, 108({out})",
            "fsw f28, 112({out})",
            "fsw f29, 116({out})",
            "fsw f30, 120({out})",
            "fsw f31, 124({out})",
            "frcsr t0", "sw t0, 128({out})", "fscsr zero",
            out = in(reg) state.as_mut_ptr(),
            inout("t2") 500_000u32 => _,
            out("t0") _,
            out("f0") _,
            out("f1") _,
            out("f2") _,
            out("f3") _,
            out("f4") _,
            out("f5") _,
            out("f6") _,
            out("f7") _,
            out("f8") _,
            out("f9") _,
            out("f10") _,
            out("f11") _,
            out("f12") _,
            out("f13") _,
            out("f14") _,
            out("f15") _,
            out("f16") _,
            out("f17") _,
            out("f18") _,
            out("f19") _,
            out("f20") _,
            out("f21") _,
            out("f22") _,
            out("f23") _,
            out("f24") _,
            out("f25") _,
            out("f26") _,
            out("f27") _,
            out("f28") _,
            out("f29") _,
            out("f30") _,
            out("f31") _,
            options(nostack),
        );
    }
    state
}

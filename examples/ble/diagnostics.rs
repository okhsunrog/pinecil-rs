//! Target-side flight recorder and opt-in timing experiments.
use core::{
    cell::UnsafeCell,
    ptr::{read_volatile, write_volatile},
};
const RECORDS: usize = 240;
#[repr(C)]
struct State {
    // magic, next, frozen, last_irq, irq_count, printf_mode, last_tag, late_store_us
    header: [u32; 8],
    records: [[u32; 8]; RECORDS],
}
struct Storage(UnsafeCell<State>);
// Single hart: every writer is serialized by the machine interrupt mask.
unsafe impl Sync for Storage {}
#[unsafe(no_mangle)]
#[unsafe(link_section = ".ble_trace")]
static BLE_FLIGHT_RECORDER: Storage = Storage(UnsafeCell::new(State {
    header: [0; 8],
    records: [[0; 8]; RECORDS],
}));
// One-shot experiment command, written through RAM while using the 144 MHz
// profile. Never change the flash clock by reading GLB through JTAG SBA.
#[unsafe(no_mangle)]
static mut BLE_FLASH_MHZ_REQUEST: u32 = 0;
unsafe extern "C" {
    static ble_ke_event_env: u32;
    fn __real_rwble_isr();
    fn __real_rwip_schedule();
    fn __real_ea_finetimer_isr();
    fn __real_ea_sw_isr();
}
pub fn init() {
    unsafe {
        let p = BLE_FLIGHT_RECORDER.0.get();
        core::ptr::write_bytes(p, 0, 1);
    }
}
pub fn activate() {
    unsafe {
        (*BLE_FLIGHT_RECORDER.0.get()).header[0] = 0x424c4554;
    }
}
pub fn mode() -> u32 {
    unsafe {
        read_volatile(core::ptr::addr_of!(
            (*BLE_FLIGHT_RECORDER.0.get()).header[5]
        ))
    }
}
#[inline(never)]
#[unsafe(link_section = ".text.ble_trace_fast")]
pub fn record(tag: u32, aux: u32) {
    critical_section::with(|_| unsafe {
        let p = BLE_FLIGHT_RECORDER.0.get();
        if read_volatile(core::ptr::addr_of!((*p).header[0])) != 0x424c4554
            || read_volatile(core::ptr::addr_of!((*p).header[2])) != 0
        {
            return;
        }
        let now = bl706_hal::time::now_ticks() as u32;
        if tag == 1 {
            (*p).header[3] = now;
            (*p).header[4] = (*p).header[4].wrapping_add(1);
        }
        let cycle: u32;
        core::arch::asm!("csrr {}, mcycle", out(reg) cycle);
        // Read-only status/raw-status/snapshot registers used by the vendor ISR.
        // Reading does not acknowledge IRQs or trigger a new clock snapshot.
        let entry = [
            tag,
            now,
            cycle,
            read_volatile(0x28000010 as *const u32),
            read_volatile(0x28000014 as *const u32),
            read_volatile(0x2800001c as *const u32),
            read_volatile(core::ptr::addr_of!(ble_ke_event_env)),
            aux,
        ];
        let n = (*p).header[1];
        write_volatile(
            core::ptr::addr_of_mut!((*p).records[n as usize % RECORDS]),
            entry,
        );
        (*p).header[6] = tag;
        write_volatile(core::ptr::addr_of_mut!((*p).header[1]), n.wrapping_add(1));
    });
}
pub fn poll() {
    let request = unsafe { read_volatile(core::ptr::addr_of!(BLE_FLASH_MHZ_REQUEST)) };
    if matches!(request, 4 | 8 | 48) {
        unsafe {
            write_volatile(core::ptr::addr_of_mut!(BLE_FLASH_MHZ_REQUEST), 0);
            flash_clock_test(request);
        }
        record(51, request);
    }
    record(30, 0);
    critical_section::with(|_| unsafe {
        let p = BLE_FLIGHT_RECORDER.0.get();
        if (*p).header[4] > 20
            && (bl706_hal::time::now_ticks() as u32).wrapping_sub((*p).header[3]) > 1_000_000
        {
            write_volatile(core::ptr::addr_of_mut!((*p).header[2]), 1);
        }
    });
}
// 48 MHz matches SDK GLB_Set_SF_CLK(1, GLB_SFLASH_CLK_96M, 1). The slower
// profiles use XCLK/8 or XCLK/4. DLL outputs are enabled by 144 MHz startup.
// Every instruction executes in ITCM while
// serial-flash clocking is temporarily disabled; IRQs remain masked throughout.
#[inline(never)]
#[unsafe(link_section = ".text.ble_trace_fast")]
unsafe fn flash_clock_test(mhz: u32) {
    let profile = match mhz {
        4 => 0x4700,  // XCLK, divide by 8.
        8 => 0x4300,  // XCLK, divide by 4.
        48 => 0x3100, // DLL96M, divide by 2.
        _ => return,
    };
    critical_section::with(|_| unsafe {
        const CFG: *mut u32 = 0x40000008 as *mut u32;
        const ENABLE: u32 = 1 << 11;
        const SELECTORS_AND_DIVIDER: u32 = 0xf700;
        let old = read_volatile(CFG);
        write_volatile(CFG, old & !ENABLE);
        let next = (old & !(SELECTORS_AND_DIVIDER | ENABLE)) | profile;
        write_volatile(CFG, next);
        write_volatile(CFG, next | ENABLE);
        core::arch::asm!("fence iorw, iorw", "fence.i");
    });
}
// Diagnostic-only call site replacing the timer register store in ea_prog_timer.
// That function saves its return address on entry. Preserve every other caller
// register. header[7] optionally injects one late store for a near deadline.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.ble_trace_fast")]
unsafe extern "C" fn ble_trace_timer_store() {
    core::arch::naked_asm!(
        "addi sp, sp, -64", "sw ra, 0(sp)",
        "sw a0, 4(sp)", "sw a1, 8(sp)", "sw a2, 12(sp)", "sw a3, 16(sp)",
        "sw a4, 20(sp)", "sw a6, 24(sp)", "sw a7, 28(sp)",
        "sw t0, 32(sp)", "sw t1, 36(sp)", "sw t2, 40(sp)",
        "sw t3, 44(sp)", "sw t4, 48(sp)", "sw t5, 52(sp)", "sw t6, 56(sp)",
        "mv a0, s2", "call {sample}",
        "lw ra, 0(sp)", "lw a0, 4(sp)", "lw a1, 8(sp)",
        "lw a2, 12(sp)", "lw a3, 16(sp)", "lw a4, 20(sp)",
        "lw a6, 24(sp)", "lw a7, 28(sp)",
        "lw t0, 32(sp)", "lw t1, 36(sp)", "lw t2, 40(sp)",
        "lw t3, 44(sp)", "lw t4, 48(sp)", "lw t5, 52(sp)", "lw t6, 56(sp)",
        "lui a5, 0x28000", "addi sp, sp, 64", "ret",
        sample = sym timer_written,
    );
}
#[inline(never)]
#[unsafe(link_section = ".text.ble_trace_fast")]
extern "C" fn timer_written(target: u32) {
    unsafe {
        let p = BLE_FLIGHT_RECORDER.0.get();
        let delay = read_volatile(core::ptr::addr_of!((*p).header[7]));
        if delay != 0 {
            write_volatile(0x2800001c as *mut u32, 0x80000000);
            while read_volatile(0x2800001c as *const u32) & 0x80000000 != 0 {
                core::hint::spin_loop();
            }
            let now = read_volatile(0x2800001c as *const u32) & 0x7ffffff;
            if target.wrapping_sub(now) & 0x7ffffff <= 4 {
                write_volatile(core::ptr::addr_of_mut!((*p).header[7]), 0);
                record(41, delay);
                let start = bl706_hal::time::now_ticks();
                while bl706_hal::time::now_ticks().wrapping_sub(start) < u64::from(delay.min(5000))
                {
                    core::hint::spin_loop();
                }
            }
        }
        write_volatile(0x280000f8 as *mut u32, target);
        // Do not refresh the native clock snapshot on the passive path: doing
        // so changes the timing/state under investigation. Fault injection may
        // refresh it to demonstrate directly that the target is already late.
        if delay != 0 {
            write_volatile(0x2800001c as *mut u32, 0x80000000);
            while read_volatile(0x2800001c as *const u32) & 0x80000000 != 0 {
                core::hint::spin_loop();
            }
        }
    }
    record(40, target);
}
/// Relate the free-running radio clock to mtime after advertising is enabled.
/// A later passive store record can be compared with this anchor without
/// refreshing the native snapshot inside ea_prog_timer's critical window.
#[inline(never)]
#[unsafe(link_section = ".text.ble_trace_fast")]
pub fn anchor() {
    critical_section::with(|_| unsafe {
        write_volatile(0x2800001c as *mut u32, 0x80000000);
        while read_volatile(0x2800001c as *const u32) & 0x80000000 != 0 {
            core::hint::spin_loop();
        }
        record(42, read_volatile(0x28000020 as *const u32));
    });
}
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.ble_trace_fast")]
unsafe extern "C" fn __wrap_rwble_isr() {
    record(1, 0);
    unsafe {
        __real_rwble_isr();
    }
    record(2, 0);
}
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.ble_trace_fast")]
unsafe extern "C" fn __wrap_rwip_schedule() {
    record(3, 0);
    unsafe {
        __real_rwip_schedule();
    }
    record(4, 0);
}
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.ble_trace_fast")]
unsafe extern "C" fn __wrap_ea_finetimer_isr() {
    record(5, 0);
    unsafe {
        __real_ea_finetimer_isr();
    }
    record(6, 0);
}
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.ble_trace_fast")]
unsafe extern "C" fn __wrap_ea_sw_isr() {
    record(7, 0);
    unsafe {
        __real_ea_sw_isr();
    }
    record(8, 0);
}

//! Standalone XIP/FPU check, with heater drive held low and RTT output only.
#![no_std]
#![no_main]

use embassy_time::{Duration, Instant, Timer};
use static_cell::StaticCell;
#[cfg(feature = "fpu")]
#[path = "support/fpu.rs"]
mod fpu;

static EXECUTOR: StaticCell<bl706_hal::Executor> = StaticCell::new();
static TICKS: bl706_hal::portable_atomic::AtomicU32 = bl706_hal::portable_atomic::AtomicU32::new(0);
use bl706_hal::portable_atomic::Ordering;

const fn pattern(i: usize) -> u32 {
    (i as u32).wrapping_mul(0x9e37_79b9).rotate_left(7) ^ 0x1357_2468
}
const fn flash_data() -> [u32; 8192] {
    let mut data = [0; 8192];
    let mut i = 0;
    while i < data.len() {
        data[i] = pattern(i);
        i += 1;
    }
    data
}
static FLASH_DATA: [u32; 8192] = flash_data();
static mut DATA_COOKIE: u32 = 0x1234_abcd;
#[unsafe(no_mangle)]
pub static mut HARDWARE_CHECK: [u32; 8] = [0; 8];

#[embassy_executor::task]
async fn ticker() {
    loop {
        Timer::after_millis(1).await;
        TICKS.fetch_add(1, Ordering::Relaxed);
    }
}

#[embassy_executor::task]
async fn check(mut heater: bl706_hal::gpio::Output<21>) {
    Timer::after_millis(20).await;
    for pass in 0..100u32 {
        let pc: u32;
        unsafe {
            core::arch::asm!("auipc {}, 0", out(reg) pc);
        }
        let data_addr = FLASH_DATA.as_ptr() as usize;
        assert!((0x2300_0000..0x2310_0000).contains(&pc));
        assert!((0x2300_0000..0x2310_0000).contains(&data_addr));
        assert_eq!(
            unsafe { (&raw const DATA_COOKIE).read_volatile() },
            0x1234_abcd
        );
        let mut checksum = 0u32;
        for n in 0..FLASH_DATA.len() {
            let i = (n * 4051) & (FLASH_DATA.len() - 1);
            let value = unsafe { FLASH_DATA.as_ptr().add(i).read_volatile() };
            assert_eq!(value, pattern(i));
            checksum = checksum.wrapping_add(value);
        }
        heater.set_low();
        let ticks = TICKS.load(Ordering::Relaxed);
        assert!(ticks > 0);
        unsafe {
            HARDWARE_CHECK = [pass + 1, pc, data_addr as u32, checksum, ticks, 0, 0, 0];
        }
        rtt_target::rprintln!(
            "XIP PASS pass={} pc={:08x} rodata={:08x} checksum={:08x} ticker={} us={}",
            pass,
            pc,
            data_addr,
            checksum,
            ticks,
            Instant::now().as_micros()
        );
        #[cfg(feature = "fpu")]
        {
            fpu::arithmetic();
            // Register a fresh deadline immediately before blocking in assembly.
            // The earlier ticker deadline may already have fired during the
            // flash-data sweep; this one guarantees an IRQ inside the FP test.
            let mut alarm = core::pin::pin!(Timer::after_millis(1));
            core::future::poll_fn(|cx| {
                let _ = core::future::Future::poll(alarm.as_mut(), cx);
                core::task::Poll::Ready(())
            })
            .await;
            let state = fpu::registers();
            for (n, actual) in state[..32].iter().copied().enumerate() {
                assert_eq!(actual, 0x3f80_0000 + n as u32, "FPR {}", n);
            }
            assert_eq!(state[32], 0x61, "FCSR changed across interrupt");
            let mstatus: usize;
            unsafe {
                core::arch::asm!("csrr {}, mstatus", out(reg) mstatus);
            }
            assert_eq!((mstatus >> 13) & 3, 3);
            unsafe {
                HARDWARE_CHECK[5] = pass + 1;
                HARDWARE_CHECK[6] = state[32];
            }
            rtt_target::rprintln!(
                "FPU PASS pass={} f32=40500000 f0..f31=OK fcsr={:02x} FS={}",
                pass,
                state[32],
                (mstatus >> 13) & 3
            );
            alarm.await;
        }
        Timer::after(Duration::from_millis(100)).await;
    }
    rtt_target::rprintln!("HARDWARE CHECK PASS");
    loop {
        heater.set_low();
        Timer::after_secs(1).await;
    }
}

#[unsafe(no_mangle)]
extern "C" fn __bl706_main() -> ! {
    rtt_target::rtt_init_print!(rtt_target::ChannelMode::NoBlockSkip, 4096);
    let p = bl706_hal::init_with_config(bl706_hal::clock::Config::pinecil_v2_144mhz())
        .expect("Pinecil clock initialization failed");
    let heater = p.pins.p21.into_output(false);
    let flash_cfg = bl706_hal::pac::GLB.clk_cfg2().read().0;
    assert_eq!(flash_cfg & 0xff00, 0x3900, "DLL96M/2 flash clock");
    assert_eq!(p.clocks.cpu_hz(), 144_000_000);
    assert_eq!(p.clocks.bus_hz(), 72_000_000);
    assert_eq!(p.clocks.flash_hz(), 48_000_000);
    let read_command = bl706_hal::pac::SF_CTRL.sf_if_iahb_1().read().0;
    let read_control = bl706_hal::pac::SF_CTRL.sf_if_iahb_0().read().0;
    rtt_target::rprintln!(
        "Clocks CPU=144 bus=72 flash=48 MHz; CLK_CFG2={:08x} IAHB={:08x}/{:08x}",
        flash_cfg,
        read_control,
        read_command
    );
    bl706_hal::enable_interrupts();
    rtt_target::rprintln!("XIP hardware check starting");
    EXECUTOR.init(bl706_hal::Executor::new()).run(|s| {
        s.spawn(ticker().unwrap());
        s.spawn(check(heater).unwrap());
    })
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    unsafe {
        core::arch::asm!("csrci mstatus, 8");
    }
    rtt_target::rprintln!("HARDWARE CHECK FAIL: {}", info);
    loop {
        core::hint::spin_loop();
    }
}

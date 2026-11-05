//! Minimal BLE controller bring-up. No heater, OLED or PD policy tasks.
#![no_std]
#![no_main]
// Formatting integers/Debug inside rprintln's critical section delays radio
// IRQs. Build the line with interrupts enabled; only copying it to RTT is locked.
macro_rules! trace {
    ($($args:tt)*) => { $crate::trace(core::format_args!($($args)*)) };
}
fn trace(args: core::fmt::Arguments<'_>) {
    #[cfg(feature = "ble-diagnostics")]
    if diagnostics::mode() == 1 {
        diagnostics::record(20, 0);
        rtt_target::print_impl::write_fmt(0, args);
        rtt_target::rprint!("\n");
        diagnostics::record(21, 0);
        return;
    }
    use core::fmt::Write;
    let mut text = heapless::String::<512>::new();
    let _ = text.write_fmt(args);
    let _ = text.push('\n');
    #[cfg(feature = "ble-diagnostics")]
    diagnostics::record(20, 0);
    rtt_target::rprint!(text.as_str());
    #[cfg(feature = "ble-diagnostics")]
    diagnostics::record(21, 0);
}
#[cfg(feature = "ble-diagnostics")]
#[path = "ble/diagnostics.rs"]
mod diagnostics;
#[path = "ble/gatt.rs"]
mod gatt;
use bl706_hal::{clock, gpio::Output};
use bt_hci::transport::PacketKind;
use embassy_time::{Duration, with_timeout};
use static_cell::StaticCell;
static EXECUTOR: StaticCell<bl706_hal::Executor> = StaticCell::new();

#[embassy_executor::task]
async fn heartbeat() {
    loop {
        #[cfg(feature = "ble-diagnostics")]
        {
            diagnostics::poll();
        }
        embassy_time::Timer::after_secs(5).await;
        trace!(
            "BLE metrics us={} {:?}",
            embassy_time::Instant::now().as_micros(),
            bl706_ble::take_metrics()
        );
    }
}

#[embassy_executor::task]
async fn worker(mut runner: bl706_ble::Runner) {
    runner.run().await
}
#[embassy_executor::task]
async fn exercise(transport: bl706_ble::NativeTransport, _heater_off: Output<21>) {
    for opcode in [0x0c03u16, 0x1001, 0x1002, 0x1003, 0x2002, 0x2003, 0x1009] {
        let [lo, hi] = opcode.to_le_bytes();
        transport.send_raw(PacketKind::Cmd, &[lo, hi, 0]).unwrap();
        let packet = with_timeout(Duration::from_secs(3), transport.receive_raw())
            .await
            .expect("HCI timeout");
        trace!("HCI {:04x}: {:02x?}", opcode, packet.bytes());
        assert_eq!(
            &packet.bytes()[..2],
            &[0x0e, packet.bytes().len() as u8 - 2]
        );
        assert_eq!(&packet.bytes()[3..5], &[lo, hi]);
        assert_eq!(packet.bytes()[5], 0, "HCI command failed");
    }
    gatt::run(transport).await;
}
#[unsafe(no_mangle)]
extern "C" fn __bl706_main() -> ! {
    #[cfg(feature = "ble-diagnostics")]
    diagnostics::init();
    rtt_target::rtt_init_print!(rtt_target::ChannelMode::NoBlockSkip, 4096);
    trace!("Pinecil BLE experiment; heater OFF; XIP/IMFC");
    let mut p = bl706_hal::init_with_config(clock::Config::pinecil_v2_144mhz()).unwrap();
    let off = p.pins.p21.into_output(false);
    bl706_hal::enable_interrupts();
    let (transport, runner) =
        unsafe { bl706_ble::initialize(p.radio, &mut p.software_interrupt) }.unwrap();
    #[cfg(feature = "ble-diagnostics")]
    diagnostics::activate();
    EXECUTOR.init(bl706_hal::Executor::new()).run(|s| {
        s.spawn(worker(runner).unwrap());
        s.spawn(heartbeat().unwrap());
        s.spawn(exercise(transport, off).unwrap());
    })
}
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    unsafe {
        core::arch::asm!("csrci mstatus, 8");
    }
    trace!("BLE PANIC: {}", info);
    loop {
        core::hint::spin_loop();
    }
}

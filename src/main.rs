#![no_std]
#![no_main]

mod board;
mod buttons;
mod control;
mod display;
mod heartbeat;
mod heater_io;
#[cfg(feature = "heater-diagnostics")]
mod heater_test;
mod pd;
#[cfg(feature = "pd-diagnostics")]
mod pd_trace;
mod rtt_commands;

use bl706_hal::i2c::I2c;
use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use pinecil_rs::app::{self, StateWatch};
use static_cell::StaticCell;

static EXECUTOR: StaticCell<bl706_hal::Executor> = StaticCell::new();
static PD_EXECUTOR: bl706_hal::InterruptExecutor = bl706_hal::InterruptExecutor::new();
static I2C_BUS: StaticCell<Mutex<CriticalSectionRawMutex, I2c>> = StaticCell::new();
static APP_STATE: StateWatch<CriticalSectionRawMutex> = StateWatch::new();
pub type SharedI2c = I2cDevice<'static, CriticalSectionRawMutex, I2c>;

#[embassy_executor::task]
async fn application() {
    app::run(
        &buttons::EVENTS,
        pd::STATUS.receiver().unwrap(),
        control::MEASUREMENTS.receiver().unwrap(),
        &APP_STATE,
    )
    .await;
}

#[cfg(feature = "ble")]
#[embassy_executor::task]
async fn ble_controller(mut runner: bl706_ble::Runner) {
    runner.run().await;
}

#[cfg(feature = "ble")]
#[embassy_executor::task]
async fn ble_host(transport: bl706_ble::NativeTransport) {
    pinecil_rs::ble::run(transport, |_| {}).await;
}

#[unsafe(no_mangle)]
extern "C" fn __bl706_main() -> ! {
    let b = board::board();
    let output = heater_io::HeaterOutput::new(b.heater, b.heater_timer);
    // The radio is initialized but not advertising. Run deliberate polling
    // stalls before starting its worker/host or the PD executor.
    let timer_ok = output.self_test();
    let bus = I2C_BUS.init(Mutex::new(b.i2c));
    // PD preempts framebuffer rendering and thread-mode formatting. Async
    // I2C retains shared bus ownership; no blocking wait is used in the IRQ.
    PD_EXECUTOR
        .start(b.software_interrupt)
        .spawn(pd::run(I2cDevice::new(bus), b.pd_irq).unwrap());
    EXECUTOR.init(bl706_hal::Executor::new()).run(|s| {
        #[cfg(feature = "ble")]
        {
            s.spawn(ble_controller(b.ble.1).unwrap());
            s.spawn(ble_host(b.ble.0).unwrap());
        }
        s.spawn(heartbeat::run(b.log).unwrap());
        s.spawn(rtt_commands::run(b.commands).unwrap());
        s.spawn(
            control::run(
                b.adc,
                output,
                b.tip_probe,
                b.watchdog,
                APP_STATE.receiver().unwrap(),
                timer_ok,
            )
            .unwrap(),
        );
        s.spawn(buttons::run(b.key_plus, b.key_minus).unwrap());
        s.spawn(application().unwrap());
        s.spawn(
            display::run(
                I2cDevice::new(bus),
                b.oled_reset,
                APP_STATE.receiver().unwrap(),
            )
            .unwrap(),
        );
    })
}

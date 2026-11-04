#![no_std]
#![no_main]

use bl706_hal::{gpio, i2c::I2c};
use core::fmt::Write;
use embassy_time::{Instant, Timer};
use embedded_graphics::{
    mono_font::{MonoTextStyle, ascii::FONT_6X10},
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::{Baseline, Text},
};
mod board;
use board::{Logger, banner, board};
use ssd1306::{I2CDisplayInterface, Ssd1306Async, prelude::*};
use static_cell::StaticCell;

static EXECUTOR: StaticCell<bl706_hal::Executor> = StaticCell::new();

#[embassy_executor::task]
async fn heartbeat(mut log: Logger, mut heater_off: gpio::Output<21>) {
    banner(&mut log, "display");
    let mut tick = 0u32;
    loop {
        heater_off.set_low();
        let _ = writeln!(
            log,
            "heartbeat tick={tick} us={}\r",
            Instant::now().as_micros()
        );
        tick = tick.wrapping_add(1);
        Timer::after_secs(1).await;
    }
}

#[embassy_executor::task]
async fn display(i2c: I2c, reset: gpio::Pin<3>) {
    let mut reset = reset.into_output(false);
    // Rotate0 uses A1/C8 and columns 0..95, matching IronOS's flipped layout.
    let mut oled = Ssd1306Async::new(
        I2CDisplayInterface::new(i2c),
        DisplaySize96x16,
        DisplayRotation::Rotate0,
    )
    .into_buffered_graphics_mode();
    oled.reset(&mut reset, &mut embassy_time::Delay)
        .await
        .unwrap();
    Timer::after_millis(100).await;
    let initialized = async {
        oled.init().await?;
        // Match the board's long precharge period and modest contrast in IronOS.
        oled.set_brightness(Brightness::custom(0xf, 26)).await
    }
    .await;
    if let Err(error) = initialized {
        rtt_target::rprintln!("OLED init failed: {:?}; heartbeat continues", error);
        return;
    }
    rtt_target::rprintln!("OLED SSD1306 96x16 initialized at I2C 0x3c");
    let text = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let mut frame = 0u32;
    loop {
        oled.clear(BinaryColor::Off).unwrap();
        Text::with_baseline("Embassy BL706", Point::zero(), text, Baseline::Top)
            .draw(&mut oled)
            .unwrap();
        Rectangle::new(
            Point::new((frame.wrapping_mul(4) % 94) as i32, 13),
            Size::new(3, 3),
        )
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(&mut oled)
        .unwrap();
        let start = Instant::now();
        if let Err(error) = oled.flush().await {
            rtt_target::rprintln!(
                "OLED frame={} failed: {:?}; heartbeat continues",
                frame,
                error
            );
            return;
        }
        if frame.is_multiple_of(4) {
            rtt_target::rprintln!(
                "OLED frame={} flushed in {} us",
                frame,
                start.elapsed().as_micros()
            );
        }
        frame = frame.wrapping_add(1);
        Timer::after_millis(250).await;
    }
}

#[unsafe(no_mangle)]
extern "C" fn __bl706_main() -> ! {
    let b = board();
    EXECUTOR.init(bl706_hal::Executor::new()).run(|s| {
        s.spawn(heartbeat(b.log, b.heater_off).unwrap());
        s.spawn(display(b.i2c, b.oled_reset).unwrap());
    })
}

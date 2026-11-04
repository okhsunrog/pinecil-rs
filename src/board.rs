use bl706_hal::{gpio, i2c::I2c};
use core::fmt::Write;

pub struct Logger;

impl core::fmt::Write for Logger {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        rtt_target::rprint!("{}", text);
        Ok(())
    }
}

pub struct Board {
    pub log: Logger,
    pub heater_off: gpio::Output<21>,
    pub i2c: I2c,
    pub oled_reset: gpio::Pin<3>,
}

pub fn board() -> Board {
    rtt_target::rtt_init_print!(rtt_target::ChannelMode::NoBlockSkip, 4096);
    rtt_target::rprintln!("Pinecil Rust firmware: flash -> RAM; initializing HAL");
    let p = bl706_hal::init();
    let heater_off = p.pins.p21.into_output(false);
    let board = Board {
        log: Logger,
        heater_off,
        i2c: I2c::new(p.i2c, p.pins.p10, p.pins.p11, p.clocks),
        oled_reset: p.pins.p3,
    };
    bl706_hal::enable_interrupts();
    board
}

pub fn banner(log: &mut Logger, name: &str) {
    let misa: usize;
    unsafe { core::arch::asm!("csrr {}, misa", out(reg) misa) };
    let _ = writeln!(log, "Pinecil {name}: nominal RC32M, misa={misa:08x}");
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    unsafe { core::arch::asm!("csrci mstatus, 8") };
    rtt_target::rprintln!("panic: {}", info);
    loop {
        core::hint::spin_loop();
    }
}

# Pinecil Rust firmware

Application firmware for Pinecil V2 / BL706, Rust 2024 and Embassy.

```sh
cd ~/code/rust/pinecil-rs
nix develop
cargo run -r
```

The runner prepares a bootable flash ELF, programs it with verification through
the patched probe-rs, resets the target and attaches RTT. No UART is required.
Code and constants execute directly from mapped SPI flash (XIP). The bootstrap
copies only startup/trap code, the vector table and initialized data into RAM.
The OLED uses async `ssd1306` and `embedded-graphics`; buttons use GPIO IRQs and
Embassy debounce/hold timers.

This is an initial firmware project: GPIO21 stays low throughout operation.
There is no heater control or temperature measurement. USB-PD requests fixed
20 V if offered, otherwise 5 V, with a 3 A ceiling. For an EPR-capable source,
it then enters EPR mode and requests a fixed 28 V profile, also limited to 3 A.
Profiles above 28 V and PPS/AVS are never selected.

## Controls

- Short `+`/`-`: change the displayed setpoint by 5 C, between 150 and 450 C.
- Hold `+` for 700 ms: open/close the brightness menu.
- Short `+`/`-` in the menu: select one of four brightness levels.

The setpoint is UI state only; the heater remains OFF. The footer shows the
PD state or the brightness controls.

## Target and memory layout

The default target is `riscv32imfc-unknown-none-elf`, using the single-precision
hardware FPU and `ilp32f` ABI. There is no A or D extension in the application.
The pinned nightly builds `core` from `rust-src` because IMFC has no distributed
prebuilt Rust core. `cargo run -r` selects this configuration automatically.

`pinecil.x` maps application text/rodata at `0x23000000` (physical flash payload
offset `0x1000`), RAM startup/vector sections at `0x22014000`, and initialized
data/BSS/8 KiB stack in OCRAM at `0x22020000`. The HAL initializes FS/fcsr and
preserves all 32 floating-point registers plus fcsr across interrupts.

## Local dependencies

Keep `../bl706-embassy` beside this repository. Its local `bl706-hal` provides
the flash bootstrap, interrupt handling and the I2C cancellation fix. This project
owns the XIP linker script, board setup, UI/PD policy, Cargo runner and boot
header. The header disables cache burst wrap for the validated SPI read mode.

Keep `../fusb302b` beside this repository: the driver is a local path dependency
so PHY fixes can be tested directly. `usbpd` is pinned to the official 3.1.0 Git
release revision `b1f0bb9`, with `epr` enabled. That release was not yet available
from crates.io when selected. A traits patch unifies the Git policy engine's
traits with the driver's registry dependency.

The installed probe-rs must include BL706 flash support (validated revision
`4884829b5` in `okhsunrog/probe-rs`, branch `fix/jtagprobe-bl706`). The Raspberry Pi
Pico runs the `okhsunrog/grapple-probe` port at `29bf9c1`.

## Wiring

| Pinecil JTAG adapter | Pico |
|---|---|
| JTMS | GP14 |
| JTCK | GP19 |
| JTDI | GP18 |
| JTDO | GP21 |
| NJTRST | GP15 |
| GND | GND |

OLED: 96×16, I²C address `0x3c`, GPIO10/11 and active-low reset on GPIO3.
FUSB302B: `0x22` on the same bus, INT_N GPIO16. Nominal I²C rate is 300 kHz,
with SCL synchronization and a one-cycle deglitch filter. Its VBUS sense pin is
not connected on Pinecil V2; use the driver's external-detector constructor.
Buttons: GPIO25/28, active high with pull-downs. GPIO21 is the active-high heater
drive and is held low.

The stock panel's full addressable resolution is 96×16. The application uses
`DisplaySize96x16`, a 192-byte framebuffer and coordinates x=0..95, y=0..15.
Blank space between the setpoint, OFF label and PD information is part of the
current UI layout. The glass is larger than the active pixel area; the display
controller's larger RAM does not add physical pixels. See the
[PINE64 specifications](https://pine64.org/documentation/Pinecil/_full/#specifications)
and [panel drawing](https://files.pine64.org/doc/datasheet/pinecil/1810010328_UG-Univision-Semicon-UG-9616TSWCG02_C88335.pdf).

## Validation

```sh
bash tools/check.sh
```

Build and ELF checks do not establish physical operation. Hardware results are
recorded in `docs/validation.md` as the application is brought up.
The board/PD source comparison and known limitations are in `docs/pd-audit.md`.
EPR traces, timing changes and the I2C cancellation regression are recorded in
[`docs/epr-bringup.md`](docs/epr-bringup.md).

`cargo run -r --example hardware-check` flashes a standalone XIP/FPU test that
checks mapped code, a 32 KiB flash constant, initialized data, f32 arithmetic,
all FPRs/fcsr and concurrent Embassy timer interrupts. It replaces the UI image;
run `cargo run -r` afterwards to reinstall the application.

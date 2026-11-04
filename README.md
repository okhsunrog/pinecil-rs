# Pinecil Rust firmware

Application firmware for Pinecil V2 / BL706, Rust 2024 and Embassy.

```sh
cd ~/code/rust/pinecil-rs
nix develop
cargo run -r
```

The runner prepares a bootable flash ELF, programs it with verification through
the patched probe-rs, resets the target and attaches RTT. No UART is required.
Boot ROM runs a small XIP bootstrap, which copies the application into RAM.
The OLED uses the async `ssd1306` crate and `embedded-graphics`.

This is an initial firmware project: GPIO21 stays low throughout operation.
There is no heater control, temperature measurement or USB-PD negotiation.

## Local dependencies

Keep `../bl706-embassy` checked out beside this repository. `bl706-hal` is a path
dependency, currently validated at commit `b2931ae`; its runtime owns the flash
bootstrap and linker script. This project owns board setup, display and UI, the
Cargo runner and boot-header preparation. The latter disables cache burst wrap
for ordinary SPI reads, as required by the validated board configuration.

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
GPIO21 is the active-high heater drive and is held low.

## Validation

```sh
bash tools/check.sh
```

Build and ELF checks do not establish physical operation. Hardware results are
recorded in `docs/validation.md` as the application is brought up.

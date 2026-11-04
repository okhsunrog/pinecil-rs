# Hardware validation

2026-11-04: Pinecil V2 / BL706, standalone Grapple Probe Pico, JTAG 1 MHz,
USB power, RTT logs, heater tip removed.

The standalone repository's initial `cargo run -r --locked` programmed and
verified flash, booted the application, initialized the 96×16 OLED and produced
fresh heartbeat ticks0/1 and OLED frames0/4. The payload was 34,864 bytes.
The initial source/build checks, clippy, ELF layout/instruction audit and five
boot-header format/rejection tests passed. These results cover the migrated
flash/display baseline; buttons and USB-PD are subsequent steps.

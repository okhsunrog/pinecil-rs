# Hardware validation

2026-11-04: Pinecil V2 / BL706, standalone Grapple Probe Pico, JTAG 1 MHz,
USB power, RTT logs, heater tip removed.

The standalone repository's initial `cargo run -r --locked` programmed and
verified flash, booted the application, initialized the 96×16 OLED and produced
fresh heartbeat ticks0/1 and OLED frames0/4. The payload was 34,864 bytes.
The initial source/build checks, clippy, ELF layout/instruction audit and five
boot-header format/rejection tests passed. These results cover the migrated
flash/display baseline; buttons and USB-PD are subsequent steps.

## XIP, FPU and IMFC

The application now defaults to `riscv32imfc-unknown-none-elf`, rebuilding
`core` with the pinned nightly. Text/rodata execute from mapped SPI flash;
startup/trap entry/vector tables and writable data retain their RAM sections.

The isolated IMC hardware check completed 100 passes while reading a 32 KiB
permuted volatile flash constant (checksum `ad3ff00d`) and running an Embassy
1 ms timer. PC `230003aa` and constant address `230035a8` were in XIP.

The IMFC check completed 100 XIP passes and 100 FPU passes, checking runtime
multiply/add/divide/fsqrt, 32 distinct FPR sentinels, fcsr `0x61` and FS=Dirty.
To test the interrupt context rather than only compiler-generated arithmetic,
a separate GDB run forced MTIME pending inside the sentinel loop, stopped at
`bl706_irq_dispatch` before its Rust prologue and verified all 32 saved FPR
words on the IRQ stack. It then zeroed every live FPR and fcsr inside the ISR.
After returning, the program still validated the original sentinels and fcsr
and completed all 100 passes. Saved mepc `23000a44` was inside the test loop.
Final RAM result was `[64,230003a2,23003d5c,ad3ff00d,25ad,64,61,0]` in hex;
the trap record remained zero. This is physical FPU/IRQ evidence.

The IMFC UI application was programmed with verify and booted its OLED, PD and
heartbeat tasks. Earlier hardware acceptance confirmed readable OLED output,
working async keys/menu and brightness at nominal I2C 300 kHz. A later cold
connection of the final XIP+PD build to the charger booted the UI, negotiated
20 V / 3 A and produced fresh RTT heartbeat. The user confirmed the display
and buttons still worked. This describes the earlier SPR-only policy.

On 2026-11-05, the normal XIP/IMFC application automatically negotiated 20 V,
entered EPR and requested PDO8 at 28 V / 3 A. KM003C captured the request, Accept,
PS_RDY and subsequent KeepAlive exchanges. A settled 180-second window contains
180,000 consecutive ADC samples with no sequence gaps: 27.892181-27.910736 V.
The user confirmed the PD 28 V display, moving indicator and buttons worked
without screen corruption. This is negotiation/idle validation with the heater
OFF, not an 84 W load test. See [epr-bringup.md](epr-bringup.md) for details.

For PD wire traces and meter measurements, see [the integration audit](pd-audit.md).

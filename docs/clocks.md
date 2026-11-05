# Pinecil V2 clocks and flash reads

The application and `hardware-check` example use
`clock::Config::pinecil_v2_144mhz()`:

| Domain | Configuration |
| --- | --- |
| CPU / FCLK | XTAL32M -> DLL144M, no CPU division: 144 MHz |
| Peripheral bus / BCLK | CPU / 2: 72 MHz |
| Flash clock | DLL96M / 2: 48 MHz |
| Flash read | Dual Output, 1-1-2, opcode `0x3B`, eight dummy clocks |
| Embassy timebase | BCLK / 72: 1 MHz |
| ADC clock | XCLK32M / 4 / 4: 2 MHz |
| I2C | Recalculated from BCLK, requested 300 kHz |

144 MHz is the [device family's rated CPU maximum](https://en.bouffalolab.com/product/?id=8&type=detail).
The flash frequency matches the [official C SDK](https://github.com/bouffalolab/bl_iot_sdk/blob/edcf613f073d3ef7d2f290d7c285b69be35da75b/components/platform/hosal/bl702_hal/bl_sys.c#L328).
The preserved stock image's flash header already selected Dual Output (`ioMode`
`0x11`), while the earlier Rust header used single-line reads (`0x10`).

The MX25L8006-compatible profile uses two existing SPI signal lines for data,
with the command/address still sent on one line. It needs no Quad Enable status
write or extra board wiring. The [Macronix MX25L8006E specification](https://www.macronix.com/Lists/Datasheet/Attachments/8680/MX25L8006E%2C%203V%2C%208Mb%2C%20v1.6.pdf)
lists up to 80 MHz for Dual Output and 86 MHz for single-line fast reads, and
does not provide Quad I/O. A QSPI-capable MCU alone does not establish that the
fitted flash supports quad transfers. This board is validated at 48 MHz; higher
rates and quad transfers are not enabled or claimed as tested.

## Boot and clock transitions

The ROM boot header retains RC32M / 8 MHz flash startup, with CRC/SHA checks.
The bootstrap copies clock transitions and interrupt startup into ITCM before
calling Rust. HAL initialization first selects XCLK / 4 for flash, so a warm
entry cannot lose flash access when the DLL is reconfigured. It then starts
the crystal/DLL, configures the bus divider before the fast CPU root, and
finally selects DLL96M / 2 for flash. The switch runs entirely in ITCM with
interrupts masked, no flash fetches, no stack accesses and an instruction fence.
Application code and constants continue executing through XIP after the switch.

`Clocks::flash_hz()` reports the configured flash clock. The general HAL default
remains CPU/bus 32 MHz and flash 8 MHz. The separate RAM-only flash programming
driver still temporarily selects its own 8 MHz command clock.

## Hardware validation, 2026-11-05

Pinecil V2 BL706, Grapple Probe Pico/JTAG, Type-A USB power, heater GPIO21 held low:

- Prepared flash image programmed with `probe-rs download --verify`, then booted
  through a core reset.
- `hardware-check` completed 100 passes over a 32 KiB XIP data pattern, larger
  than the instruction/data cache, with each value checked independently.
- 100 f32 arithmetic/all-register context checks passed across timer interrupts.
  The Embassy ticker continued running at the recalculated timebase.
- Halted PROGBUF readback confirmed DLL CPU selection, CPU divisor 1, bus divisor
  2, flash `CLK_CFG2=0xff8f3910`, and read command `0x3b000000`.
- Heater output was low with its output-enable bit set.
- The normal application booted at this clock profile on 5 V USB, initialized
  FUSB302B and continued heartbeat and VIN/handle/tip measurements with zero
  reported ADC errors and the heater disabled.

[Final register and test state](clocks-hardware.json) records the measurements.
These initial clock-only tests establish operation on this board at room
conditions, not a voltage/temperature qualification. Later charger-powered
startup and combined 28 V PD/BLE/UI results are in [ble-integration.md](ble-integration.md).

# Combined UI, ADC, PD and BLE firmware

BLE is enabled in the default application. `cargo run --release --locked`
programs one image containing OLED/buttons, read-only measurements, USB-PD and
the shared Trouble GATT counter service. Heating remains disabled. GATT writes
only change the test counter; application settings/telemetry mapping is future
work.

## Integration

- CPU/bus/flash remain 144/72/48 MHz, with Dual Output flash reads and urgent
  controller code in ITCM.
- Board initialization holds GPIO21 low, completes RF calibration with IRQ3
  still exclusively available, then configures ADC/I2C. Only afterward does
  `main` start the PD software interrupt executor.
- Radio IRQ72 preempts thread tasks. The native controller worker and Trouble
  host share the thread executor with UI/ADC tasks. No additional BLE interrupt
  executor is used.
- The application and isolated HCI example share `src/ble.rs`. RTT formatting
  stays outside the channel's critical section. Timing maxima are logged every
  five seconds.
- Data/BSS may continue from ITCM into lower OCRAM, ending before RF scratch
  at `0x22024000`. The stack uses `0x22028000..0x2202a000`. Optional trace buffers
  use `0x2202a000..0x2202e000`; the top 8 KiB remain BLE exchange memory. Linker
  assertions and ELF audits enforce these boundaries.
- The large optional PD trace is NOLOAD and explicitly initialized before
  radio/PD startup. Native BLE dependencies are target-specific so host tests
  do not try to build the RISC-V HAL for the host.

## Hardware results, 2026-11-05

The tested 328,608-byte boot payload has SHA256
`3c893ed114882182b080a7be32fea0913c80a0ed3af580406ffad037e82062da`.
The final default build produces the same payload. Controller/HAL source is
pinned to the [main-branch merge](https://github.com/okhsunrog/bl706-embassy/commit/1457dd6b21c3f43a3daa822076499f4581c1c491).
Programming used `probe-rs download --verify` over Grapple Probe Pico/JTAG.

The initial 5 V boot initialized BLE, FUSB302B, OLED and ADC successfully. During
the first GATT test session the user changed power to the EPR charger. That
session completed nine cycles and then a connection timed out; it is excluded
from the uninterrupted results below. The available trace does not establish
the exact relationship between that timeout and the power change.

On the fresh charger-powered boot, RTT recorded a 20 V / 3 A SPR contract at
406,384 us and a 28 V / 3 A EPR contract at 837,669 us. With power/cables then
unchanged:

- Ten complete discovery/read/write/readback/notify/disconnect cycles passed,
  delivering 80 sequential notifications. OpenOCD was closed throughout.
- The user exercised `+`/`-`, LIGHT and the measurement page during this run and
  confirmed no display blanking, power interruption or UI hang.
- A further 30-second connection passed with 30 notifications while RTT was
  recorded. UI, measurements and EPR maintenance continued in the same image.
- The last 24 recorded sensor reports span 324.294..353.044 seconds since boot.
  ADC input estimates remained 27,725..27,758 mV, with zero ADC errors and
  `heater=OFF`.
- In captured five-second windows, observed maxima were 183 us for the native
  radio ISR, 1,281 us for native scheduling and 1,485 us from wake notification
  to worker execution. These are sampled observations, not worst-case bounds.

[Numeric results and selected samples](ble-integration-hardware.json) accompany
the report. VIN here is the board's ADC estimate; this session did not include
an independent KM003C PD/voltage capture. Pairing, encryption, phones, OTA,
sleep, sustained throughput and long-duration reliability remain untested.

## Checks

`tools/check.sh` validates formatting, 20 host state/UI/power tests, Clippy,
boot-header integrity tests and ELF placement/ABI/atomic/FPU checks. It covers
the combined default image, PD diagnostics, a build without BLE, the hardware
check example and isolated BLE diagnostic ITCM/XIP variants. CI runs those
offline checks; the physical tests above are separate evidence.

# Pinecil BLE experiment

The `experiment/ble-controller` branch runs the official BL70x peripheral-only
controller with an Embassy worker and Trouble 0.8.0. It does not include FreeRTOS.
The HAL/controller dependency is pinned to a public
[Git revision](https://github.com/okhsunrog/bl706-embassy/commit/9689d70da70e790362a46cdbc8ac0019a0944736);
no sibling checkouts or local patches are needed. Its
[port notes](https://github.com/okhsunrog/bl706-embassy/blob/f3059ecbc936c67aa01204f58c040aabdf8bb84c/docs/ble-port.md)
cover the binary ABI, software-IRQ calibration hook, entropy and memory layout.

## Run

The BLE archive build additionally requires GNU `ar` and `nm` (binutils).
They are included in the Nix shell and available on the CI runner.

Use the normal Pico/JTAG wiring from the README. Power Pinecil from a simple
5 V Type-A USB source. This image holds GPIO21 low and never enables heating.
OLED, buttons, ADC and PD tasks are not started; an old OLED frame may remain
visible until the display is reset or power is removed.

```sh
git switch experiment/ble-controller
cargo run --release --locked --features ble --example ble
```

The runner prepares the boot header, programs flash with verification and
attaches RTT. The image boots and executes through XIP, using the IMFC target.
It first verifies native HCI commands, then advertises as **Pinecil Rust**.

The experiment uses the 144 MHz CPU / 72 MHz bus profile referenced to XTAL32M.
Early 32 MHz images could transfer GATT data but also timed out during connection
establishment. A 144 MHz image with all controller code in XIP passed an initial
three-cycle run, then a repeated test stopped advertising after two cycles.
CPU frequency alone was insufficient. Moving urgent controller paths into ITCM
passed five reconnect/read/write/notify cycles and reduced observed ISR maxima
from about 7 ms to about 1 ms in those runs.

`pinecil-ble.x` places urgent `rwble/rwip/lld/ea` and helper code in ITCM along
with writable data/BSS. The 8 KiB stack uses lower OCRAM at
`0x22020000..0x22022000`, outside RF scratch `0x42024000..0x42028000` and exchange
memory. Most host/application code continues to execute through XIP. The build
script selects the linker script by absolute path: a same-named script in the
working directory cannot shadow it.
Vendor `.bleromro` and initialized data are included in the startup copy layout;
CI rejects orphan sections, overlap with RF scratch, RTOS symbols and A opcodes.
The default UI firmware continues to use `pinecil.x`.

## GATT smoke test

Use the laptop's BLE adapter, with BlueZ running and Bluetooth powered on:

```sh
uv run tools/test_ble.py
```

This script uses pinned inline Python dependencies installed by uv. It discovers
the device by advertised name, connects three times, checks service discovery,
reads/writes a four-byte little-endian counter, verifies readback and subscribes
to one-second notifications before disconnecting. It is a hardware test, not a
CI test. Only one device with this experimental name/address should be powered
nearby during the test.

| Item | UUID / format |
| --- | --- |
| Test service | `b1706000-0000-4000-8000-000000000001` |
| Counter characteristic | `b1706001-0000-4000-8000-000000000001` |
| Characteristic operations | Read, write with response, notify |
| Value | u32 little endian; increments once per second while connected |
| Temporary random-static address | `C2:00:00:0B:07:06` |

This is a test counter, not a setpoint or heater control. The address is fixed
for reproducibility; per-device identity/privacy is future work.

See the [timer/placement investigation](ble-forensics.md) for captured failure
state, controlled deadline injection, timer-only recovery and the C SDK reference.

## Validation scope

The 2026-11-05 hardware session used Pinecil V2, the Grapple Pico JTAG port and
the laptop's BlueZ/Bleak central. HCI Reset, local version, supported commands,
LMP/LE features, LE buffers and address commands returned status zero. The
controller reported two 251-byte ACL buffers. Both directions of ACL traffic
worked through Trouble: service discovery, reads, writes and notifications.

The example reports per-window radio ISR, C scheduling and worker-wakeup timing
maxima over RTT. Millisecond outliers also occurred without live JTAG. The initial diagnostic
rprintln calls formatted Debug/numbers with IRQs masked. Logs now format into a
local buffer with IRQs enabled, then only copy to RTT inside its lock. These
measurements are not a proven scheduling budget for UI/PD coexistence.

The current ITCM/logging image uses a 144 MHz CPU, 72 MHz bus and 48 MHz Dual
Output (1-1-2) flash reads, as described in [clocks.md](clocks.md). It completed
ten reconnect/read/write/notification cycles and 80 notifications with OpenOCD
closed throughout the test. Its 206,368-byte boot payload has SHA256
`72737f97d007186117721b42c28081ebd299e9b5010d91b687fcacf1a5dfd41e`.
Post-test register readback confirmed `CLK_CFG2=0xff8f3910`, CPU/bus divisors
1/2 and flash read opcode `0x3B`; GPIO21 remained driven low.
After a further `probe-rs reset --method core`, the same image passed three
more complete cycles and 24 notifications without reflashing or USB reconnection.
Flash programming used probe-rs `--verify`; offline checks pass for both the
normal application and the BLE example. These short tests do not establish
long-term radio reliability.

Not yet tested: phones, pairing/bonding/encryption, OTA, sleep, or simultaneous
OLED/PD workload. BLE remains a standalone experiment. No heating code is run.

To restore the normal UI/sensor/PD application:

```sh
cargo run --release --locked
```

A longer single-connection run can be requested with:

```sh
uv run tools/test_ble.py --cycles 1 --seconds 60
```

# pinecil-rs

[![CI](https://github.com/okhsunrog/pinecil-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/okhsunrog/pinecil-rs/actions/workflows/ci.yml)

Rust 2024 / Embassy firmware for **Pinecil V2**, powered by the Bouffalo BL706.
The firmware boots from SPI flash, executes code through XIP, uses the
single-precision hardware FPU, and exposes logs over RTT through JTAG.

The default firmware runs BLE alongside the UI, measurements and USB-PD.
It uses the official peripheral-only controller without FreeRTOS, with an
Embassy worker and Trouble. See [BLE setup, GATT test and limits](docs/ble.md).

The current application includes:

- A 96×16 OLED UI using async [`ssd1306`](https://crates.io/crates/ssd1306)
  and [`embedded-graphics`](https://crates.io/crates/embedded-graphics).
- Async GPIO buttons with Embassy debounce and hold timers.
- BLE advertising and a read/write/notify counter service. Application settings
  and telemetry are not yet mapped to GATT characteristics.
- ADC estimates of input voltage, handle temperature and tip
  temperature, with raw counts and faults logged over RTT.
- Tip-resistance probing and a separate temperature/power controller, with
  timer-limited heater bursts, latched faults and a watchdog. Initial limits
  are 20 W estimated average power and a 350 C maximum setpoint.
- USB-PD sink negotiation through [`fusb302b`](https://github.com/okhsunrog/fusb302b)
  and [`usbpd`](https://github.com/elagil/usbpd): 20 V SPR followed by 28 V EPR
  when offered, with a 3 A request limit. Otherwise it uses 5 V.
- A Cargo runner that prepares the boot header, programs flash with readback
  verification, resets the core and attaches RTT.

**Heating starts OFF after every boot.** Holding both buttons for one second
requests heating; pressing both again stops it. Sensor values are nominal
estimates, not independently calibrated hot-tip temperatures. See the
[heater design, interlocks and qualification status](docs/heating.md).
Flashing this project replaces the firmware already installed on the iron,
including IronOS. The earlier BLE/PD validation was performed with heating off.

PD runs on a software interrupt executor, preempting UI work while I2C
interrupts can preempt PD. This addresses the missed EPR response deadlines
seen with the earlier shared thread executor. Hardware measurements and the
remaining recoverable OLED FIFO fault are documented in the
[timing investigation](docs/pd-scheduling.md).

Radio IRQ72 preempts thread tasks; the BLE controller worker and Trouble host run
on the thread executor. RF calibration finishes before ADC/I2C initialization
and before PD takes ownership of software IRQ3.

Application state and input handling run independently of OLED drawing and
recovery. PD and application snapshots use multi-reader watches; a busy display
receives the latest state without blocking input processing. See the
[task ownership and state flow](docs/architecture.md).

## Development hardware

The maintainer uses a **Raspberry Pi Pico (RP2040) as the JTAG debug probe**.
The tested setup consists of:

- A Pinecil V2 / BL706.
- The [PINECIL Break Out Board](https://pine64.com/product/pinecil-break-out-board/),
  which exposes the iron's JTAG signals. This adapter is required for the
  documented external connection; see also the
  [PINE64 development documentation](https://pine64.org/documentation/Pinecil/_full/).
- A Pico running the [Grapple Probe Pico port](https://github.com/okhsunrog/grapple-probe/tree/port/raspberry-pi-pico),
  plus jumper wires and a USB cable to the development computer.
- A USB-C power source and cable for the Pinecil, connected through the breakout
  board. A non-EPR 5 V USB source is convenient for flashing and debugging.
- For PD testing, a suitable charger and cable. Testing 28 V requires an
  EPR-capable source and cable.

A [KM003C](https://github.com/okhsunrog/km003c-rs) was used to record PD traffic
and voltage during bring-up; it is optional for normal development.

### Pico firmware and JTAG wiring

Build the port's **`pico`** binary, convert it to UF2, and copy it to the Pico's
RPI-RP2 volume while in BOOTSEL mode. The complete commands are in the
[Pico port instructions](https://github.com/okhsunrog/grapple-probe/blob/29bf9c12285c2d6a261319ce7e48338604f9ad18/grapple-probe-firmware/Readme.md#raspberry-pi-pico-rp2040);
that [revision](https://github.com/okhsunrog/grapple-probe/commit/29bf9c12285c2d6a261319ce7e48338604f9ad18)
was used for hardware validation.

Connect the breakout board to the Pico as follows. GP numbers are Pico GPIO
labels, not physical header pin numbers.

| Pinecil breakout signal | Pico GPIO | Purpose |
|---|---|---|
| JTMS | GP14 | JTAG state control |
| JTCK | GP19 | JTAG clock |
| JTDI | GP18 | Data from probe to target |
| JTDO | GP21 | Data from target to probe |
| NJTRST | GP15 | JTAG TAP reset |
| GND | GND | Common ground |

The Pico connects to the computer over USB as a CMSIS-DAP v2 probe. The Pinecil
gets power through its own USB-C connection; this setup connects only JTAG
signals and ground to the Pico. The Pico port uses direct **3.3 V GPIO**.
NJTRST resets the JTAG TAP; the tested wiring has no separate target nRESET.
Pico GP16 is unused here, and GP20 is reserved by the probe firmware and left
unconnected. The runner uses **JTAG at 1 MHz**.

RTT logs travel over the same JTAG connection. UART wiring is unnecessary.

## Build from a standalone clone

Cargo fetches the HAL and PAC from public Git repositories and
[`fusb302b 0.2.0`](https://crates.io/crates/fusb302b/0.2.0) from crates.io.
Every Git dependency is pinned to a revision, and `Cargo.lock` is committed.
Only this repository needs to be cloned; the build does not use sibling
checkouts or local path overrides.

```sh
git clone https://github.com/okhsunrog/pinecil-rs.git
cd pinecil-rs
```

### With Nix

The [development shell](flake.nix) supplies the pinned Rust toolchain, Python,
uv, Git, GNU binutils, LLVM and OpenOCD:

```sh
nix develop
cargo build --release --locked
```

### With rustup

Install [Rust through rustup](https://rustup.rs/),
[`uv`](https://docs.astral.sh/uv/getting-started/installation/) and Git, then run:

```sh
rustup show active-toolchain
cargo build --release --locked
```

rustup reads [rust-toolchain.toml](rust-toolchain.toml) and installs the pinned
nightly with `rust-src`, rustfmt, Clippy and LLVM tools. Cargo builds `core`
from source for the tier-3 `riscv32imfc-unknown-none-elf` target. There is no
separate `rustup target add` step for IMFC.

Building and running the offline checks does not require a probe or Pinecil.

## Install the host debug tool

Flashing requires the [patched probe-rs fork](https://github.com/okhsunrog/probe-rs/tree/fix/jtagprobe-bl706)
with BL706 target detection and SPI flash programming support. The tested
[revision](https://github.com/okhsunrog/probe-rs/commit/4884829b503b29a3519c1e424402a6c4ec437d95)
can be installed with:

```sh
cargo install --locked --force --bin probe-rs \
  --git https://github.com/okhsunrog/probe-rs \
  --rev 4884829b503b29a3519c1e424402a6c4ec437d95 probe-rs-tools
probe-rs list
```

This is a separate host tool; the Nix shell does not install it automatically.
On Linux, set up USB permissions using the
[probe-rs udev instructions](https://probe.rs/docs/getting-started/probe-setup/#linux-udev-rules).

## Flash and read RTT logs

With the Pico connected to the computer, the JTAG wires connected and the
Pinecil powered, run:

```sh
cargo run --release --locked
```

The [Cargo runner](tools/run.py) prepares a bootable `.flash.elf`, writes it
through probe-rs, verifies the contents, resets the core and reads RTT logs.
Code and constants execute directly from mapped flash. Only startup/trap code,
the vector table and initialized data are copied into RAM. The image persists
across power loss and boots without holding a button.

Run the same command again to install the next build. The normal JTAG workflow
does not require the ROM USB bootloader or a button press.

When debugging an active **EPR** contract, halting the CPU also stops the PD
KeepAlive task. The source can then reset VBUS and interrupt a debug or flash
operation. Use a non-EPR 5 V USB source for programming. The diagnostic
`pd-manual-start` feature leaves PD disabled after boot until explicitly started
through the debugger; see [the EPR bring-up notes](docs/epr-bringup.md).

For intermittent PD faults, build with `--features pd-diagnostics`. PD starts
automatically, while RTT channel 1 carries fixed 44-byte binary records of FIFO
transactions, I2C errors and cancelled operations. Channel 0 retains text logs.
Both channels use `NoBlockSkip`; an absent reader cannot block the firmware.
With OpenOCD running, capture both channels without resetting the target:

```sh
uv run --no-project tools/pd_diagnostics.py --seconds 300 \
  --stream-output pd-i2c.jsonl > pd-rtt.log
```

The trace captures at most 32 data bytes per transaction. A cancelled bus future
does not prove that the slave's FIFO position was preserved; correlate the trace
with a PD sniffer and a voltage time series. `pd-manual-start` also enables this
diagnostic transport.

CPU timing records use address zero; stages are documented in
[the timing investigation](docs/pd-scheduling.md). On a stable 5 V source,
`--verbose-heartbeat 1` temporarily enables the old verbose heartbeat for
profiling; restore the ordinary short log with `--verbose-heartbeat 0`.

To prepare a bootable ELF without accessing hardware:

```sh
uv run --no-project tools/flash_image.py \
  target/riscv32imfc-unknown-none-elf/release/pinecil-rs
```

This writes `target/riscv32imfc-unknown-none-elf/release/pinecil-rs.flash.elf`.
The ordinary ELF remains available for symbols and RTT decoding.

## Controls and display

| Input | Action |
|---|---|
| Short `+` / `-` | Change the setpoint by 5 °C, within 150–350 °C |
| Hold both buttons for 1 s while OFF | Request heating |
| Press both buttons while heating | Stop heating; holding them longer keeps it OFF |
| Hold `+` for 700 ms | Open or close the LIGHT brightness menu |
| Short `+` / `-` in LIGHT | Choose one of four brightness levels |
| Hold `-` for 700 ms | Enter or leave the measurements page |

The measurements page shows `IN` (ADC input-voltage estimate), `H` (handle
temperature) and `TIP` (tip-temperature estimate). Short presses do not change
settings on that page. Saturated/invalid sensors show an error instead of a
stale temperature. See [conversion details and validation status](docs/measurements.md).

The footer shows PD status, such as `PD 28V 3.0A`, or menu controls. A moving
square indicates ongoing UI updates. The displayed current is the negotiated
limit, not measured load current. `ON` / `REG` indicate heating/regulation;
`ERR` indicates a latched controller fault. Menus keep the current heating
request; use both buttons to stop. After a fault, switch OFF before restarting.

The full addressable panel is **96×16 pixels**, with a 192-byte framebuffer.
The header keeps its 6×10 glyphs with blank padding clipped to eight rows;
the footer uses a 5×8 font in the other eight rows. The controller's larger RAM
does not add physical pixels. See the
[panel drawing](https://files.pine64.org/doc/datasheet/pinecil/1810010328_UG-Univision-Semicon-UG-9616TSWCG02_C88335.pdf)
and [PINE64 specifications](https://pine64.org/documentation/Pinecil/_full/#specifications).

## CPU target, atomics and memory layout

The default target is **`riscv32imfc-unknown-none-elf`**, with the `ilp32f` ABI.
F32 operations use the hardware FPU; the D extension and hardware f64 operations
are excluded. The HAL enables floating-point state before entering Rust and
preserves all 32 floating-point registers plus `fcsr` across interrupts.
XIP and FPU/interrupt-context checks have run on the physical BL706; see
[hardware validation](docs/validation.md).

### Why the A extension is excluded

The observed `misa` advertises A, but that bit alone does not establish that
the chip's memory bus supports the atomic operations Rust can generate.
A [Bouffalo discussion about the related BL702](https://bbs.bouffalolab.com/d/133-atomic-memory-operations)
reports AMO access faults on RAM despite `misa.A`; the reply confirms the bus
limitation. Rust atomic read-modify-write and compare/exchange operations can
emit AMO or LR/SC instructions when compiling for a target with A.

This firmware therefore uses an IMFC target that excludes A, with
`portable-atomic` falling back to interrupt-masking critical sections on the
single hart. The ELF audit rejects A instructions in the application and
hardware-check example. This is a conservative target choice: LR/SC and AMO
have **not** been validated on this BL706. The
[HAL atomic investigation](https://github.com/okhsunrog/bl706-embassy/blob/27de879918f48f4c95131da8afe81dc894e3551b/docs/atomics.md)
records the evidence, limitations and isolated test procedure.

### Flash and RAM

| Region | Use |
|---|---|
| Physical flash offset `0x0000` | ROM boot header with CRC/hash fields |
| Physical flash offset `0x1000` | Application payload |
| XIP at `0x23000000` | Application code and read-only data |
| ITCM `0x22014000..0x22020000` | Startup, trap/vector sections, urgent BLE code and the first data/BSS portion |
| Lower OCRAM `0x22020000..0x22024000` | Remaining data/BSS |
| OCRAM `0x22024000..0x22028000` | Reserved RF calibration scratch |
| OCRAM `0x22028000..0x2202a000` | 8 KiB stack |
| OCRAM `0x2202a000..0x2202e000` | Optional diagnostic trace buffers |
| OCRAM `0x2202e000..0x22030000` | BLE exchange memory |

The application owns [pinecil-ble.x](pinecil-ble.x), the board setup, UI/PD policy,
Cargo runner and [boot-header preparation](tools/flash_image.py). The boot header
selects Dual Output reads (1-1-2, command `0x3B`) and disables cache burst wrap,
matching the mode in this board's stock header. After conservative ROM startup,
the firmware selects a 144 MHz CPU, 72 MHz bus and 48 MHz flash clock.
See [clock configuration and hardware checks](docs/clocks.md) for the sources,
transition sequence and validation scope.
The optional build without BLE uses [pinecil.x](pinecil.x) and the original
OCRAM layout: `cargo run --release --no-default-features --features fpu`.

## Pinned firmware dependencies

| Component | Source and validated revision | Role |
|---|---|---|
| `bl706-hal` / `bl706-pac` / `bl706-ble` | [bl706-embassy](https://github.com/okhsunrog/bl706-embassy), [synchronized timer counter revision](https://github.com/okhsunrog/bl706-embassy/commit/0362b12961ae3b208c508d9e8496c0bcd2791ac0) | Startup, interrupts, FPU context, peripherals, radio controller and Embassy integration |
| `fusb302b` | [0.2.0 on crates.io](https://crates.io/crates/fusb302b/0.2.0), [release source](https://github.com/okhsunrog/fusb302b/tree/v0.2.0) | Async FUSB302B PD PHY |
| `usbpd` / `usbpd-traits` | [official 3.1.0 revision](https://github.com/elagil/usbpd/commit/b1f0bb9720e362039ac5e5a511199c828da26c04) | PD policy engine, including EPR |

The `usbpd-traits` registry patch points to the same Git revision as the policy
engine, so the driver and engine share one traits package. The exact sources
are declared in [Cargo.toml](Cargo.toml) and recorded in [Cargo.lock](Cargo.lock).

## Board peripherals

| Peripheral | BL706 connection |
|---|---|
| OLED | I²C `0x3c`, reset GPIO3 active low |
| Shared I²C | GPIO10 SCL / GPIO11 SDA, AF6, nominal 300 kHz |
| FUSB302B | I²C `0x22`, INT_N GPIO16 active low |
| Buttons | `+` GPIO25 / `-` GPIO28, active high with pull-downs |
| Heater drive | GPIO21 / PWM1, timer-limited carrier bursts; OFF at boot |
| Cartridge probe | GPIO24, off/on/off resistance sensing with heating stopped |

The I²C setup enables SCL synchronization and a one-cycle deglitch filter.
Pinecil leaves the FUSB302B VBUS sense pin disconnected; the integration uses
the driver's external-detector constructor. The shared-bus and IRQ handling
are described in the [board/PD audit](docs/pd-audit.md).

## Checks and CI

Run the same hardware-independent checks used by
[GitHub Actions](.github/workflows/ci.yml):

```sh
bash tools/check.sh
```

This verifies that dependency resolution uses only Git/registry sources, checks
formatting, runs host UI/power tests, builds the release firmware and
hardware-check example, runs Clippy, audits XIP layout/ABI/FPU context and the
absence of A instructions, and tests boot-header generation/rejection.

CI runs on pushes, pull requests and manual dispatch from a standalone checkout.
Successful runs publish the symbol ELF and prepared flash ELF as the
`pinecil-rs-firmware` artifact. CI does not execute the firmware on hardware.

For a physical XIP/FPU test:

```sh
cargo run --release --locked --example hardware-check
```

It checks mapped code, a 32 KiB flash constant, initialized data, f32 arithmetic,
all FPRs/`fcsr` and concurrent Embassy timer interrupts. It replaces the UI image;
run `cargo run --release --locked` afterwards to reinstall the application.

Hardware results and diagnostic history:

- [Flash, OLED, buttons, XIP and FPU validation](docs/validation.md).
- [Board schematic / IronOS comparison and SPR bring-up](docs/pd-audit.md).
- [28 V EPR traces, timing and I²C cancellation investigation](docs/epr-bringup.md).

- [Combined BLE, UI, measurements and 28 V EPR validation](docs/ble-integration.md).
- [Heater control and qualification](docs/heating.md).

The default BLE image and standalone example are built, linted and audited in CI. Hardware GATT tests
require a connected Pinecil and a Bluetooth adapter and are run separately.

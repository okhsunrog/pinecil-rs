# pinecil-rs

[![CI](https://github.com/okhsunrog/pinecil-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/okhsunrog/pinecil-rs/actions/workflows/ci.yml)

Rust 2024 / Embassy firmware for **Pinecil V2**, powered by the Bouffalo BL706.
The firmware boots from SPI flash, executes code through XIP, uses the
single-precision hardware FPU, and exposes logs over RTT through JTAG.

The current application includes:

- A 96×16 OLED UI using async [`ssd1306`](https://crates.io/crates/ssd1306)
  and [`embedded-graphics`](https://crates.io/crates/embedded-graphics).
- Async GPIO buttons with Embassy debounce and hold timers.
- USB-PD sink negotiation through [`fusb302b`](https://github.com/okhsunrog/fusb302b)
  and [`usbpd`](https://github.com/elagil/usbpd): 20 V SPR followed by 28 V EPR
  when offered, with a 3 A request limit. Otherwise it uses 5 V.
- A Cargo runner that prepares the boot header, programs flash with readback
  verification, resets the core and attaches RTT.

**Heater control and temperature measurement are not implemented.** GPIO21 is
held low, the display shows OFF, and the setpoint is UI state only. Flashing
this project replaces the firmware already installed on the iron, including
IronOS. Hardware validation has been performed with the heater off.

PD scheduling under UI load is still being investigated: missed response
deadlines can trigger Hard Reset and a power cycle. The driver release does
not resolve this application issue. See the [timing investigation](docs/pd-scheduling.md)
for captures, measurements and the planned interrupt executor.

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
uv, Git, LLVM and OpenOCD:

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
| Short `+` / `-` | Change the displayed setpoint by 5 °C, within 150–450 °C |
| Hold `+` for 700 ms | Open or close the LIGHT brightness menu |
| Short `+` / `-` in LIGHT | Choose one of four brightness levels |

The footer shows PD status, such as `PD 28V 3.0A`, or menu controls. A moving
square indicates ongoing UI updates. The displayed current is the negotiated
limit, not measured load current. The heater remains off in every menu.

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
| ITCM starting at `0x22014000` | RAM-resident startup, trap entry and vector sections |
| OCRAM starting at `0x22020000` | Initialized data, BSS and 8 KiB stack |

The application owns [pinecil.x](pinecil.x), the board setup, UI/PD policy,
Cargo runner and [boot-header preparation](tools/flash_image.py). The boot header
disables cache burst wrap for the SPI read mode validated on this board.

## Pinned firmware dependencies

| Component | Source and validated revision | Role |
|---|---|---|
| `bl706-hal` / `bl706-pac` | [bl706-embassy](https://github.com/okhsunrog/bl706-embassy), [I2C cancellation fix](https://github.com/okhsunrog/bl706-embassy/commit/27de879918f48f4c95131da8afe81dc894e3551b) | Startup, interrupts, FPU context, peripherals and Embassy integration |
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
| Heater drive | GPIO21, held low |

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

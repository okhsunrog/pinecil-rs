# BLE timer and execution-placement investigation

This experiment keeps Pinecil GPIO21 driven low. It does not start heating,
OLED, PD or sensor tasks. The controller and RF archives are unchanged; the
optional instruction patch affects only a disposable diagnostic ELF.

## Established failure mechanism

On 2026-11-05 an all-XIP image reproduced a persistent advertising stop. Firmware
heartbeat continued, but the radio interrupt count stopped. The C dispatcher
returned with an empty kernel event bitmap. IRQ72 remained enabled. The event
arbiter still had a timer target, but the hardware timer was in the past:

- Timer target: `0x22be`; the last vendor clock snapshot was `0x22bd`.
- A later explicit clock capture returned `0x17db3` while the target remained
  `0x22be`. Reading the passive snapshot alone would not establish elapsed time.
- Writing only the hardware timer target to current clock + 1600 slots restored
  advertising. Without resetting/reinitializing the controller or changing its
  RAM state, three complete GATT read/write/notification/reconnect cycles passed.

Disassembly identifies `ea_prog_timer` as the relevant programming path. It
captures the native clock, corrects a too-close target to current slot + 1,
then writes the target to BLE offset `0xf8`. If the fine timer is masked, it
subsequently acknowledges the previous event and enables bit 7 at offset `0x0c`.
It does not validate the deadline again after the store/enable sequence.
One native slot is 625 microseconds. Its rounding helper adds a slot only
when the captured fine counter is below 106 microseconds (`0x6a`); the
minimum remaining time can therefore be about 106 microseconds rather than
a full slot. The selected archive contains debug
information, but not the original C source for this function.

A controlled injection reproduced the same mechanism with urgent code in ITCM:

1. Three normal GATT cycles passed with no injection.
2. A RAM command delayed exactly one near-deadline store by 2000 microseconds.
3. Before the delay the clock was `0x1a3af`, with target `0x1a3b0`.
4. Immediately after the store the captured clock was `0x1a3b2`: the deadline
   was already two slots in the past. The radio stopped generating interrupts
   while Embassy heartbeat continued for subsequent five-second windows.
5. Re-arming only the hardware timer again restored three full GATT cycles,
   without a controller reset.

[Raw final records and register snapshots](ble-timer-evidence.json) accompany
these observations. Each record is `[index, tag, mtime_us_low, mcycle_low,
intstat, intrawstat, captured_clock, kernel_event_bitmap, auxiliary]`.
Tags 1/2 bracket the radio ISR, 3/4 bracket `rwip_schedule`, 5/6 bracket the
fine-timer ISR, 7/8 bracket the software-event ISR, 10 is advertising complete,
11/12 are connect/disconnect, 20/21 bracket logging, 30 is heartbeat, 40 is a
sampled timer store, 41 begins the injected delay (auxiliary = microseconds), and 42 anchors
radio/mtime clocks (auxiliary = fine countdown).

This establishes a missed-deadline failure and recovery mechanism, rather than
assuming every connection timeout is a dead controller. It does **not** measure
the exact capture-to-store duration in the original uninstrumented XIP failure,
nor retroactively prove the cause of every earlier hang. The natural failure's
postmortem and successful recovery match the injected failure.

## Separate timing problems and observer effects

`rtt-target` 0.6.2 formats `write_fmt` arguments while holding its critical
section. Formatting Debug/numbers there delays radio service. The example
formats into a local buffer with interrupts enabled and locks only for copying.
Restoring the old formatting path caused connection-establishment timeouts.
Some such timeouts happened while radio events continued, so they must not be
reported as the persistent timer stall.

The native dispatcher and ISR execute synchronously. A latched Embassy Signal
avoids losing a notification between dispatch and waiting, but does not make
those calls preemptible or provide a scheduling budget. Millisecond delays were
observed with OpenOCD disconnected, so live JTAG polling is not a necessary cause.

Instrumentation also perturbs timing and cache placement. An instrumented
all-XIP image completed five cycles; a later repeat completed eleven before a
connection timeout, with radio events still active. This does not contradict the
uninstrumented timer stall and does not establish reliability.


## Natural late store with a slower SPI clock

A later diagnostic image added an anchor (tag 42) **after** advertising enable:
auxiliary is the captured fine-counter value, while `mtime` and the captured
radio clock establish their relationship. On its passive timer-store path the
helper does not refresh the vendor clock snapshot. There was no injected delay
(header word 7 remained zero). The refined 8 MHz build completed five GATT cycles.

The clock-sweep image, with all urgent native code in XIP, has payload SHA256
`1c35bab3139e01b023ba30e2a4a04291c715fb03a4bcb346229ee9b026570d8d`.
Its target-executed ITCM helper changed only SPI from 8 to 4 MHz. After connection
attempts, it reproduced the persistent timer stall with these records:

| Observation | Value |
| --- | --- |
| Timer target in store record 3714 | `0xa18c` (41356) |
| Store observation `mtime` | 25,974,994 us |
| Anchor record 3719 `mtime` | 25,988,671 us |
| Anchor native clock | `0xa1a2` (41378) |
| Anchor fine countdown | 592 us |

The corresponding deadline is approximately
`25988671 + (41356 - 41378) * 625 - (625 - 592) = 25974888 us`.
The store observation is therefore about **106 us late**. Sampling/quantization
and the few ITCM instructions between the store and record prevent treating
this as an exact single-microsecond measurement. They do not justify assuming
that a full slot remained available. No interrupt or clock reset occurs between
the final store and this anchor. Radio interrupts then stopped while heartbeat
continued through subsequent windows. The arbiter retained the expired target,
IRQ72 was enabled, and no radio interrupt was pending.

These are included as `natural_spi4` in the raw evidence. The original 8 MHz
postmortem, this naturally expired store, and independent one-shot injection
support the same missed-deadline mechanism. This experiment intentionally slows
the flash path; it does not establish the exact duration of every original
8 MHz failure window.

## RAM placement and the C SDK comparison

The normal Rust BLE linker places urgent `rwble_*`, `rwip_*`, `lld_*`, `ea_*`,
list/event helpers and `.bleromro` in ITCM. This consumes about 29 KiB including
startup/trampolines. Data/BSS also need ITCM; the 8 KiB stack sits in lower OCRAM.
RF calibration owns `0x42024000..0x42028000` (the OCRAM alias), and exchange
memory is reserved separately. Moving the entire SDK archive, including debug
sections or all initialization code, is neither necessary nor a valid RAM-size
estimate. Rarely used initialization/calibration and host/application code can
stay in XIP. Hardware measurements support retaining the urgent subset in ITCM.

The inspected [official C SDK clock setup](https://github.com/bouffalolab/bl_iot_sdk/blob/edcf613f073d3ef7d2f290d7c285b69be35da75b/components/platform/hosal/bl702_hal/bl_sys.c)
uses a 144 MHz CPU and `GLB_SFLASH_CLK_96M` divided by two: **48 MHz SPI**.
Its [linker](https://github.com/bouffalolab/bl_iot_sdk/blob/edcf613f073d3ef7d2f290d7c285b69be35da75b/components/platform/soc/bl702/bl702/evb/ld/flash.ld)
leaves most controller code in flash. Our conservative boot header currently
leaves ordinary single-SPI reads at **8 MHz**. CPU frequency alone therefore
was not an equivalent configuration. Comparing RTOS names without accounting
for flash frequency and code placement would mix independent variables.

## Diagnostic commands

Build with `--features ble-diagnostics --example ble`. Set
`BLE_CODE_PLACEMENT=itcm` (default) or `xip`; both reserve identical data/BSS/stack
addresses. The flight recorder occupies `0x22022000..0x22023e20`, outside RF
scratch, and uses target-side MMIO with no formatting or JTAG on its hot path.
The linker checks its upper bound. These variants are investigation tools.

`tools/ble_snapshot.py ELF OUTPUT.json` freezes the ring and reads RAM through
an already-running OpenOCD TCL server. MMIO snapshots use a halted core with
PROGBUF, then resume it. Never read GLB with JTAG system-bus access. Start
OpenOCD only after the standalone radio test when investigating a stop.

To enable the optional timer-store hook, run:

```sh
uv run --no-project tools/ble_instrument.py INPUT.elf DIAGNOSTIC.elf
uv run --no-project tools/flash_image.py DIAGNOSTIC.elf
```

The tool matches the audited instructions inside `ea_prog_timer` and replaces
them with a call to an ITCM helper. It fails on a mismatched site. The vendor
archive is not edited. Flash only the resulting `.flash.elf` with verification.
Keep a verified normal image available for restoration.

The recorder header consists of eight u32 words: magic, next index, frozen,
last IRQ time, IRQ count, printf mode, last tag, and pending late-store delay.
Mode 0 uses the corrected logging path; mode 1 restores formatting under the
RTT lock. Setting word 7 to 2000 arms the one-shot fault injection, consumed
only for a timer target at most four native slots ahead. Zero disables it.
Never enable fault injection in an application using PD or heating.

## Reproducing the C fixture

Use SDK revision `edcf613f073d3ef7d2f290d7c285b69be35da75b` and its bundled
GCC 10.2.0 toolchain. [The source patch](ble-sdk-fixture.patch) adapts the demo
to direct boot on Pinecil: heater-off pin initialization, no boot2/factory-DTS
requirement, a test MAC, no board temperature-compensation task, diagnostic
counters and the corrected ACL API spelling, and SMP-off guards/array bounds in the demo
event dispatcher. It retains the SDK FreeRTOS port,
controller worker, host stack and clock setup. This is an adapted board fixture,
not the unmodified EVB image.

Replace the SDK controller/RF archives with this experiment's exact pair from
`bl706-ble/vendor` (including the original `arch_main.o` in the C build):

- Controller SHA256 `94d05d9c87b64703bb6b8c330ea6b21a5f40258e3301324741e9141be7e2a9b5`.
- RF SHA256 `94dd54071072ea241367b094b109f97a85e731e301f94cce3946d89741893ad6`.

Build `customer_app/bl702_demo_event` with:

```sh
make CONFIG_BT=1 CONFIG_BLECONTROLLER_LIB=m0s1 CONFIG_USB_CDC=0 \
  CONFIG_BLE_PERIPHERAL_AUTORUN=1 CONFIG_BT_RESET=0 \
  CONFIG_EASYFLASH_ENABLE=0 CONFIG_SYS_VFS_UART_ENABLE=0 \
  CONFIG_SYS_AOS_CLI_ENABLE=0 -j8
```

`CONFIG_BT_RESET=0` disables a legacy host recovery extension absent from the
selected controller ABI. Start from a clean build when changing these options.
Use `tools/ble_sdk_image.py SDK.elf OUTPUT.elf` to add a real boot-header section
and physical flash offsets; flash `OUTPUT.elf.flash.elf`, not the SDK ELF.
This avoids depending on the SDK boot2/partition flashing workflow.

## C hardware reference results

The timed SDK fixture programmed with verification has a 196,080-byte payload,
SHA256 `a5dfc500481778a7ba7bc5439c2b7d899f338afb0dddd60e2afbcc16dff9a86d`.
`uv run tools/test_ble_sdk.py` completed ten connect/read/disconnect cycles,
120 ATT reads, with OpenOCD closed throughout. Native RAM counters recorded
10 connections and 10 disconnections. The firmware heartbeat continued.
An earlier fixture also completed ten cycles before the timing wrapper changed.

After the test, halted PROGBUF readback gave `CLK_CFG2=0xff8f7910`: flash enabled,
source selector 3 (96 MHz), divider 1 (divide by two). GPIO21 output was zero,
output-enable was set, and its mux was GPIO. The maximum native radio ISR
measured with `mcycle/144` since boot was 2514 microseconds. This includes startup
and is an observed maximum, not a worst-case bound or a measurement of the
specific clock-capture-to-timer-store window.

The SDK GAP name write returned insufficient authorization; the reference test
therefore checks reads and reconnection, not writable settings or notifications.
It is not the same application workload as the Trouble counter test. The C
fixture demonstrates that this same binary pair runs through the SDK C/FreeRTOS
integration on Pinecil; it cannot by itself isolate RTOS from clock/cache/layout
changes. No Zephyr image was run in this investigation.

The diagnostic `BLE_FLASH_MHZ_REQUEST` RAM word accepts 4, 8 or 48. Heartbeat
consumes the command and switches the flash clock from ITCM with IRQs masked;
4/8 MHz use XCLK dividers, 48 MHz uses DLL96M divided by two, as the SDK does.
Only the BLE example's 144 MHz profile enables the required DLL outputs. This is
an experiment command, not a general live-clock API for the normal application.
A stalled timer still needs re-arming; changing SPI does not repair an already
expired hardware target by itself.

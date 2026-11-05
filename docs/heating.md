# Tip control

Heating starts **OFF after every boot**. Hold both buttons for one second to
request heating; pressing both again stops it after debounce, without waiting
for another long hold. Ordinary setpoint, brightness and measurement-page
buttons do not start heating. A fault latches until an explicit OFF request.

The initial qualification limits are **20 W average estimated electrical power**
and a **350 C maximum setpoint**. These are firmware bring-up limits, not a
statement of cartridge or board ratings. Tip temperature remains a nominal
estimate; it has not been checked against an independent hot-tip reference.

## Design and references

The hardware timing and resistance-probe sequence are based on the Pinecil V2
board support in IronOS. Rust control and I/O are separate modules, following
the useful separation in loeti. The PI implementation is written for this
firmware in physical units, rather than copying either project's RTOS task,
JBC gains or scaled-integer regulator.

- [Pinecil V2 schematic](https://files.pine64.org/doc/Pinecil/Pinecil_schematic_v2.0_20220608.pdf).
- IronOS [PWM/ADC scheduling](https://github.com/Ralim/IronOS/blob/c7413e0d2adfd43156510d4ccbd109cf352ca335/source/Core/BSP/Pinecilv2/IRQ.cpp),
  [carrier/ADC setup](https://github.com/Ralim/IronOS/blob/c7413e0d2adfd43156510d4ccbd109cf352ca335/source/Core/BSP/Pinecilv2/Setup.cpp),
  [resistance probe](https://github.com/Ralim/IronOS/blob/c7413e0d2adfd43156510d4ccbd109cf352ca335/source/Core/BSP/Pinecilv2/BSP.cpp),
  [power calculation](https://github.com/Ralim/IronOS/blob/c7413e0d2adfd43156510d4ccbd109cf352ca335/source/Core/Src/power.cpp)
  and [regulation](https://github.com/Ralim/IronOS/blob/c7413e0d2adfd43156510d4ccbd109cf352ca335/source/Core/Threads/PIDThread.cpp).
- loeti [async control task](https://github.com/elagil/loeti/blob/03a1d9e7467bc95e475958ad677118629686eec5/firmware/loeti/common/src/control/mod.rs)
  and [tool controller](https://github.com/elagil/loeti/blob/03a1d9e7467bc95e475958ad677118629686eec5/firmware/loeti/common/src/control/tool/mod.rs).
- BL702 RM [PWM](https://github.com/bouffalolab/bl_docs/blob/665e7daa84b498a767e656f458f26aca3be4fd45/BL702_RM/en/RST/PWM.rst)
  and [TIMER registers](https://github.com/bouffalolab/bl_docs/blob/665e7daa84b498a767e656f458f26aca3be4fd45/BL702_RM/en/RST/tmr_register.rst).

`heater.rs` contains the host-testable PI, resistance classification, power
conversion, state and interlocks. `control.rs` owns the ADC, GPIO24 probe and
watchdog, receives UI requests, and reads the current PD watch directly.
`heater_io.rs` exclusively owns PWM1/GPIO21 and TIMER0 through the HAL's
interrupt driver. UI, OLED and BLE never manipulate that output.

## Timing and electrical output

Pinecil's Tip_Control drives an AC-coupled transistor/gate circuit. A static
high level is not the heating command. PWM1 is configured for a nominal
5.33 kHz, 50% carrier, enabled only during a requested burst. Average power
is controlled by the carrier's on-window, not by assuming its 50% duty
halves the power in the cartridge.

The acquisition loop targets a 100 ms period:

1. Stop the output and reject a burst which should already have ended.
2. Wait at least 10 ms with the carrier disabled for gate/amplifier settling.
3. Acquire separate finite VIN, handle and tip scans, each with a 25 ms timeout.
4. Validate the fresh frame, current power contract and cartridge state.
5. Request at most one carrier burst of 500..75000 us; shorter requests become
   zero output. A further burst requires a new valid frame.

TIMER0 is a free-running nominal 1 MHz counter. Its compare ISR disables
PWM before doing bookkeeping, then masks the compare interrupt. There is no
automatic repeat. Absolute future deadlines use wrapping 32-bit addition;
arming acknowledges a previously latched comparator flag. TIMER0 and the
watchdog preserve each other's divider fields.

The normal executor may be late without extending a burst to the next task
poll. This is an interrupt-based cutoff, **not an autonomous hardware one-shot**:
masking interrupts or a stalled core can delay it. A separate reset-mode
watchdog is configured for nominally 500 ms and is fed by the acquisition loop.
Panic explicitly forces PWM1 low before logging. These mechanisms do not
protect against a physically shorted power MOSFET.

Startup runs 20 ms and 2 ms timer checks at **zero PWM duty**, before starting
the BLE host/worker and PD executor. The 2 ms check withholds executor polling
for 6 ms while interrupts stay enabled. Failed
checks prevent live heating. A completed cutoff more than 2 ms late also
prevents further heating. These checks observe registers/timestamps, not an
oscilloscope waveform at the heater.

## Resistance and power

With heating stopped, GPIO24 injects the small probe current through R51/D15.
The probe runs at boot and before each transition from OFF to heating, so a
cartridge changed while idle is not assigned a previous cartridge's resistance.
Three readings, 100 ms apart, record off/on/off thermocouple-input voltage.
The driven reading is compared with the mean of the off readings, cancelling
a linear temperature drift. More than 200 uV off-to-off movement is rejected.
The empirical Pinecil V2 bins classify 4, 6.2 or 8 ohms and reject open/short
results; they do not provide a precise resistance measurement. Probe GPIO24
returns low on error or cancellation. It stays low during thermometry.

Estimated on-window power is `VIN² / R`. The burst duration converts desired
average power into energy per 100 ms frame, reserving the off/ADC window.
The budget is the minimum of the requested limit, 20 W, `VIN²/R` times the
maximum heating-window fraction, and measured VIN times the contract current
less a 200 mA board margin. A valid 9..28 V contract is
required; measured VIN must be 9..30 V and within 10% of it. There is no heating
from an unnegotiated 5 V USB port.

The board has no implemented heater-current feedback in this firmware. The
power estimate does not measure instantaneous current or guarantee that every
source tolerates the burst load. Resistance changes with temperature and
electrical losses are not identified by this model.

## Fault handling and remaining qualification

Missing ADC trim, invalid VIN/NTC/tip data, absent/invalid cartridge resistance,
lost power contract, stale/repeated/future samples, excessive control interval,
handle temperature outside 0..60 C or tip outside 0..420 C stop output.
While heating more than 10 C below target, at least 20 J of commanded energy
without a 3 C rise over 5 seconds latches a no-rise fault. This is a basic
response check, not a complete model of all runaway or sensor faults.

The PI uses 0.35 W/C proportional gain and 0.10 W/(C s) integral gain, with
conditional integration at the output limits. It resets on OFF, target changes
and faults. Gains, thermal limits and sampling parameters require cartridge
testing; host thermal simulation does not establish physical stability.

Absolute hot-tip calibration, independent current/power measurements, external
waveform timing, multiple cartridges, sustained soldering loads and complete
fault qualification remain separate from successful firmware builds.

## Hardware test build

`--features heater-diagnostics` adds `HEATER_TEST_COMMAND`, an eight-byte RAM
mailbox: ABI magic `0x48545231` followed by the command word. It is absent from
normal builds. A debugger may send these values **only through that RAM word**:

| Command | Action |
| --- | --- |
| 1 | 5 W limit, 80 C target, automatically OFF after 8 s |
| 2 | 10 W limit, 150 C target, automatically OFF after 30 s |
| 3 | Stop the bench request |
| 4 | Inject one invalid temperature frame |
| 5 | Stop output, withhold watchdog feeding and wait for reset |
| 6 | Withhold acquisition-task polling for 350 ms |

Bench mode stays explicitly OFF after its deadline and does not fall back to
an earlier UI heating request. Fault tests require a new OFF transition before
the next run. GPIO21 remains off during timer self-tests and watchdog testing.

With OpenOCD attached, the RTT capture tool checks the mailbox ABI and RTT
signature before sending a command. Pass the exact ELF programmed into the
device, rather than an unflashed rebuild:

```sh
uv run --no-project tools/pd_diagnostics.py --heater-command 1 --seconds 15 \
  path/to/programmed.elf > heater-test.log
```

## Validation status

Host checks cover gesture debounce/chord suppression, resistance bins, energy
limits, stale/error frames, power loss, no-rise detection, anti-windup and a
simple thermal-load simulation with a voltage change. Release/Clippy and ELF
audits cover normal, diagnostic and no-BLE builds. These are software results.

The initial physical qualification is pending: zero-duty cutoff checks,
resistance readings, watchdog reset, bounded low-power heating, fault injection
and coexistence with UI, BLE and PD. Earlier read-only ADC/PD/BLE results do
not establish that live heater control works.

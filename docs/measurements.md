# Read-only analog measurements

`control.rs` owns the ADC, the heater-off GPIO21 output and the high-impedance
GPIO24 cartridge-resistance probe input. It never constructs a PWM driver,
drives GPIO21 high or injects tip-detection current. This is the first,
read-only stage of a tip controller, not a heater regulator.

## Hardware and source references

- [Pinecil V2 production schematic](https://files.pine64.org/doc/Pinecil/Pinecil_schematic_v2.0_20220608.pdf).
- [IronOS Pinecil V2 pin mapping](https://github.com/Ralim/IronOS/blob/c7413e0d2adfd43156510d4ccbd109cf352ca335/source/Core/BSP/Pinecilv2/Pins.h)
  and [ADC setup / analog-setting investigation](https://github.com/Ralim/IronOS/blob/c7413e0d2adfd43156510d4ccbd109cf352ca335/source/Core/BSP/Pinecilv2/Setup.cpp).
- [BL706 HAL ADC contract](https://github.com/okhsunrog/bl706-embassy/blob/a2cc082defd0e35cdcecb342dd2df4c4e507834b/docs/analog.md).
- [TDK NTCG163JF103FTDS datasheet](https://product.tdk.com/system/files/dam/doc/product/sensor/ntc/chip-ntc-thermistor/data_sheet/datasheet_ntcg163jf103ftds.pdf):
  10 kohm at 25 C, B25/50 = 3380 K, rated temperature range -40..125 C.
- Numerical sensor reference points are documented by the
  [Pinecil V2 NTC model](https://github.com/Ralim/IronOS/blob/c7413e0d2adfd43156510d4ccbd109cf352ca335/source/Core/BSP/Pinecilv2/BSP.cpp),
  [tip model](https://github.com/Ralim/IronOS/blob/c7413e0d2adfd43156510d4ccbd109cf352ca335/source/Core/BSP/Pinecilv2/ThermoModel.cpp)
  and [board constants](https://github.com/Ralim/IronOS/blob/c7413e0d2adfd43156510d4ccbd109cf352ca335/source/Core/BSP/Pinecilv2/configuration.h).
  The Rust conversion routines interpolate these numerical reference data;
  the IronOS control loop is not ported.

| Input | GPIO / ADC channel | Conversion |
| --- | --- | --- |
| VIN | GPIO18 / CH8 | 120 kohm upper and 10 kohm lower divider; input voltage = pin voltage times 13. |
| Tip | GPIO19 / CH9 | Non-inverting amplifier gain = 1 + 680000/2370, then cartridge reference curve and handle compensation. |
| Handle | GPIO20 / CH10 | NTC pulls up toward 3.3 V; 10 kohm pull-down. Interpolate the NTC reference points. |

## Acquisition and state

The task samples at 4 Hz. Each channel uses a separate finite four-entry scan
with 14-bit / average16 ADC settings and the HAL's nominal 3.2 V reference. The
four gain-corrected readings are averaged in software. A scan has a 25 ms timeout;
the heater remains low throughout conversion, timeout and recovery.

The task reads VIN, then handle, then tip. Independent scans keep a saturated or
absent tip from discarding the other channels' readings. Handle temperature is
available before applying tip cold-junction compensation. The 1 ms settling
interval is a read-only starting point; future heating requires separately
qualified off-time, analog settling and sample scheduling.

ADC gain trim is read from the boot-loaded eFuse shadow. No eFuse programming
or reload is performed. An unavailable/invalid trim is logged; measurements
continue with unity gain and `gain_calibrated=false`.

Every cycle publishes a new `Measurements` snapshot with a timestamp, per-sensor
values/faults, gain-corrected raw counts, gain-trim status and a cumulative ADC
error count. Failed channels have no new count and do not retain an old valid
temperature. The latest low-level ADC error is included in the once-per-second
RTT log. These snapshots are estimates; a `Value` does not mean independently
calibrated physical accuracy.

The application receives measurements through its own watch subscription.
ADC updates do not reset settings or modify the PD contract. OLED traffic on
the main/brightness pages does not increase just because new samples arrive.
The measurements page updates from the latest snapshot.

## Display and interpretation

Hold `-` to enter or leave the measurements page:

```text
TIP 25C      OFF
IN 5.1V H25C
```

`IN` is measured VIN; the main page's `PD` voltage/current remains the negotiated
contract. `H` is handle temperature. `TIP SAT` means the input is near the ADC
rail or ADC saturation was reported; this cannot distinguish an absent tip,
wiring fault and excessive temperature. `TIP ERR` indicates another failure;
pending measurements display dashes.

## Accuracy and limitations

VIN resistor tolerances, reference error and analog configuration affect
absolute voltage accuracy. The NTC conversion assumes nominal 3.3 V and 10 kohm
pull-down; the physical 3.3 V rail is not measured.

Tip readings use a nominal 900 uV offset and a cartridge reference curve with
handle compensation. That offset is not calibration of this individual tip.
After subtracting this nominal offset, a negative thermocouple voltage is
clamped to zero; the resulting estimate equals handle temperature. Agreement
between those two values at room temperature therefore does not validate the
tip offset or prove that a cartridge is present.
The amplifier uses the resistor ratio without integer gain truncation. Near-zero
or rail-high tip inputs and out-of-range NTC inputs are rejected. An absent tip
must never be assigned the handle temperature as a valid fallback.

No cartridge resistance measurement, PID, PWM, power budget, thermal-runaway
protection or watchdog qualification is implemented. Sensor values must not be
used to enable heating until those stages have been implemented and tested.

## Validation status

Host tests cover divider scaling, nominal NTC behavior, reference interpolation,
tip offset/handle compensation, invalid sensor inputs and propagation of new
sensor faults through application state. CI also builds normal/diagnostic
images, runs Clippy and checks XIP/FPU layout and unsupported atomics.

The first hardware run on 2026-11-05 read a valid eFuse gain coefficient
(denominator 1981), but all async scans timed out. A separate manual conversion
produced four FIFO words with the configured positive channel and negative tag
23. The HAL had selected peripheral number 25 as a CLIC interrupt directly,
omitting RISC-V's IRQ base of 16. ADC needs IRQ41; the same audit corrected PWM
to IRQ50. The [HAL correction](https://github.com/okhsunrog/bl706-embassy/commit/a2cc082defd0e35cdcecb342dd2df4c4e507834b)
updates generated metadata, dispatch, startup vectors and ELF checks.

The corrected image was programmed into flash with readback verification and
started using a core reset. Independent four-entry scans of CH8, CH10 and CH9
then completed at 4 Hz without an ADC error. The boot log reported the 5 V to
20 V SPR transition, while measurements and the UI continued running.

In a separate 20-second steady-state recording, the input estimate was
20.009..20.052 V (mean 20.032 V) and handle temperature was 29.97..30.12 C.
The simultaneous KM003C capture contained 19,986 samples with no gaps:
20.084..20.095 V, mean 20.090 V. Mean voltage disagreement was about 0.3%;
this checks one operating point and does not establish full-range accuracy.
The ADC's error counter remained zero. Tip estimates equalled handle
temperature because the raw thermocouple reading was below the nominal offset;
this does not establish correct per-cartridge calibration or presence detection.

Several recoverable OLED FIFO errors occurred earlier in the session, without
an ADC fault; the steady-state recording contained none. This run does not
establish that the earlier intermittent I2C issue has been eliminated.

Remaining hardware checks include cartridge-present/absent behavior, an
independent temperature reference, cold boot of this image, cancellation/fault
injection and UI stress with simultaneous ADC and 28 V EPR traffic. Heating
remains disabled throughout.

# Read-only analog measurements

`control.rs` owns the ADC, the heater-off GPIO21 output and the high-impedance
GPIO24 cartridge-resistance probe input. It never constructs a PWM driver,
drives GPIO21 high or injects tip-detection current. This is the first,
read-only stage of a tip controller, not a heater regulator.

## Hardware and source references

- [Pinecil V2 production schematic](https://files.pine64.org/doc/Pinecil/Pinecil_schematic_v2.0_20220608.pdf).
- [IronOS Pinecil V2 pin mapping](https://github.com/Ralim/IronOS/blob/c7413e0d2adfd43156510d4ccbd109cf352ca335/source/Core/BSP/Pinecilv2/Pins.h)
  and [ADC setup / analog-setting investigation](https://github.com/Ralim/IronOS/blob/c7413e0d2adfd43156510d4ccbd109cf352ca335/source/Core/BSP/Pinecilv2/Setup.cpp).
- [BL706 HAL ADC contract](https://github.com/okhsunrog/bl706-embassy/blob/75b485167197a9d5aeb53e1d64936b861889aa08/docs/analog.md).
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

This acquisition code has not yet been run on hardware. The HAL's original ADC
documentation also marks its physical behavior unverified. Hardware checks must
verify scan tags, completion, gain trim and stable samples; compare VIN with
KM003C at 5 V and later 20/28 V, check plausible room-temperature handle/tip
readings, and test absent-tip behavior while keeping the heater disabled.

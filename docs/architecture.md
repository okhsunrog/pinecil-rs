# Task ownership and state flow

The firmware separates event processing from display I/O. This keeps button
handling and application state independent of OLED initialization, framebuffer
drawing and recovery. The PD engine still runs on the preemptible software
interrupt executor described in [PD scheduling](pd-scheduling.md).

```text
GPIO button tasks -- ordered button events --> Application task
                                                    |
PD interrupt task -- latest PowerStatus watch ------>| AppState watch
                         |                          |
                         +-- other subscribers      +--> Display task
                                                    +--> future subscribers
ADC / tip controller ------- Measurements watch ---->|
       ^                                            |
       +------------ UI request from AppState -------+
SC7A20 task ---------------- MotionState watch ------>|

Tip controller -- one bounded burst --> TIMER0/PWM1 owner
                                      | cutoff in ISR
```

## Responsibilities

| Module | Owner and responsibility |
| --- | --- |
| `board.rs` | Initializes GPIO, I2C, ADC, RTT, disabled heater PWM, cutoff timer and stopped watchdog. |
| `main.rs` | Wires shared resources and starts the thread and PD executors. |
| `buttons.rs` / `button_gestures.rs` | Own button GPIOs, debounce input and recognize short/long presses and two-button chords. |
| `app.rs` | Owns UI settings and combines buttons, power status, measurements and motion. Runs without display or GPIO dependencies and is tested on the host. |
| `ui.rs` | Implements bounded setpoint/brightness changes, rotation settings and page transitions. |
| `display.rs` | Owns the OLED/reset pin, draws snapshots, applies brightness/rotation and retries initialization or recovery after errors. |
| `accelerometer.rs` / `motion.rs` | Acquire SC7A20 raw XYZ on the shared bus; filter rotation, track movement and invalidate stale/missing motion data. No heater requests are generated. |
| `pd.rs` / `power.rs` | Own the FUSB302B/PD engine and select an allowed power contract. |
| `control.rs` / `sensors.rs` | Own ADC, resistance probe and watchdog; acquire independent scans and convert nominal readings. |
| `heater.rs` | Hardware-independent PI, power budget, resistance bins and latched interlocks. |
| `heater_io.rs` | Own PWM1/GPIO21 and TIMER0; end each carrier burst in the timer ISR. |
| `heartbeat.rs` | Emits liveness diagnostics independently of acquisition. |
| `ble.rs` | Owns the shared Trouble GATT counter service. Native radio ownership lives in the controller worker; GATT writes do not change application settings or heater state. |

The controller starts OFF. It reads UI requests from its own AppState
subscription and the latest PD status directly from the power watch. OLED or
BLE activity never owns the output. TIMER0 ends each burst without waiting for
the acquisition task; another valid ADC frame is required to rearm it.
See [tip control](heating.md) for limits, watchdog behavior and qualification.

## Events and snapshots

Button presses use an eight-element `Channel`. Events preserve order; the
application consumes them even when the display is unavailable. Producers wait
if the application itself stops draining a full queue. This is not an unlimited
input history.

PD publishes a `Watch<PowerStatus>` initialized to `Waiting`. The application
reads its current value on startup, so it also observes a contract negotiated
before the application task starts. Independent readers can observe the same
status without consuming another reader's update. The power, measurement and
application watches permit three simultaneous asynchronous readers; the motion
watch has one, owned by the application.

The controller publishes per-channel values/faults through a separate
`Measurements` watch, initialized to pending. The application is the sole writer
of `AppState`, containing `Ui`, `PowerStatus`, `Measurements` and `MotionState`. It publishes a
new snapshot only when those values change. The
display obtains the latest snapshot before each frame and after recovery;
brightness changes during recovery are reapplied from that snapshot.

SC7A20 acquisition runs at 25 Hz in HR ±2 g mode. Samples are coalesced to 4 Hz
for the application, with immediate publication of confirmed rotation changes.
The motion policy uses signed raw output words, without assuming calibrated mg.
Filtering and a 400 ms hold confirm orientation; the current raw sample must
still support the candidate, so filter lag cannot finish a transient roll.
Flat poses retain the previous orientation. Sensor errors retain orientation
but invalidate idle history; 500 ms without a fresh sample is also stale.
Recovery starts a new observation period, with no accumulated sleep time.
Manual RIGHT/LEFT settings override the automatic result. No automatic
sleep/wake or movement-triggered heating is enabled in this step.

Watches overwrite stale values without waiting for readers. They convey current
state, not a lossless fault/event history. A slow display may skip intermediate
setpoints or power transitions and must not serve as the future safety monitor.
Negotiated voltage/current describe the PD contract. ADC VIN is a separate
measurement estimate; actual heater current/consumption is not measured.

## Scheduling and bus access

Buttons, application, ADC controller, accelerometer, display, heartbeat, native BLE worker and
Trouble host run on the thread executor. Radio IRQ72 preempts these tasks. The
application waits for input or power changes and performs only short state
updates. It does not render, access I2C or wait for a display acknowledgement.

PD runs on the software interrupt executor. Both executors share the existing
async I2C mutex using `CriticalSectionRawMutex`. Peripheral interrupts can
preempt PD; no task blocks synchronously waiting for bus ownership. OLED retains
the yields between rendering stages and between individual I2C transactions.

This separation does not change PD policy, clocks, I2C frequency, the HAL or
the FUSB302B driver. Shared-bus latency and recoverable FIFO errors still require
hardware observation; moving state processing cannot establish their absence.

## Validation

`tools/check.sh` runs host tests for state updates with a stalled display,
independent/late subscribers, and ignored events that should not redraw. It
also builds normal and diagnostic firmware, runs Clippy, and audits XIP/FPU
and unsupported atomic instructions.

The SC7A20 integration was flashed with readback verification on 15 V SPR,
with heating OFF. Fresh XYZ and increasing idle time were observed while five
BLE discovery/read/write/notify/disconnect cycles passed. ADC reported no errors
and the 15 V contract was still ready after the run. These checks establish
acquisition/BLE coexistence; they do not establish physical display orientation
or qualify automatic sleep/wake or operation on EPR.

The application/display separation and read-only controller have been flashed
with readback verification. Correcting the HAL's ADC interrupt to CLIC41 allowed
the 4 Hz async scans to complete without an ADC error. At 20 V SPR, a simultaneous
20-second KM003C capture agreed with measured VIN within about 0.3% on average.
See the [acquisition contract and limits](measurements.md).
The earlier hardware results in [PD scheduling](pd-scheduling.md) apply to the
previous image. The [combined BLE firmware session](ble-integration.md) records
the later charger-powered startup, 28 V EPR maintenance, GATT traffic and manual
button/menu/measurement-page stress. Recoverable OLED FIFO errors were observed
in earlier sessions; one successful stress run does not establish long-term
absence of those faults.

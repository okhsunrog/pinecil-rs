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

Tip controller -- one bounded burst --> TIMER0/PWM1 owner
                                      | cutoff in ISR
```

## Responsibilities

| Module | Owner and responsibility |
| --- | --- |
| `board.rs` | Initializes GPIO, I2C, ADC, RTT, disabled heater PWM, cutoff timer and stopped watchdog. |
| `main.rs` | Wires shared resources and starts the thread and PD executors. |
| `buttons.rs` / `button_gestures.rs` | Own button GPIOs, debounce input and recognize short/long presses and two-button chords. |
| `app.rs` | Owns UI settings and combines buttons, power status and measurements. Runs without display or GPIO dependencies and is tested on the host. |
| `ui.rs` | Implements bounded setpoint/brightness changes and page transitions. |
| `display.rs` | Owns the OLED/reset pin, draws snapshots, applies brightness and retries initialization or recovery after errors. |
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
status without consuming another reader's update. Each watch currently permits
three simultaneous asynchronous readers.

The controller publishes per-channel values/faults through a separate
`Measurements` watch, initialized to pending. The application is the sole writer
of `AppState`, containing `Ui`, `PowerStatus` and `Measurements`. It publishes a
new snapshot only when those values change. The
display obtains the latest snapshot before each frame and after recovery;
brightness changes during recovery are reapplied from that snapshot.

Watches overwrite stale values without waiting for readers. They convey current
state, not a lossless fault/event history. A slow display may skip intermediate
setpoints or power transitions and must not serve as the future safety monitor.
Negotiated voltage/current describe the PD contract. ADC VIN is a separate
measurement estimate; actual heater current/consumption is not measured.

## Scheduling and bus access

Buttons, application, ADC controller, display, heartbeat, native BLE worker and
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

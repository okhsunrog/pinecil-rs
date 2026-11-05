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
ADC / heater-off controller -- Measurements watch -->|
```

## Responsibilities

| Module | Owner and responsibility |
| --- | --- |
| `board.rs` | Initializes GPIO, I2C, ADC, RTT logging and the heater-off output. |
| `main.rs` | Wires shared resources and starts the thread and PD executors. |
| `buttons.rs` | Owns button GPIOs, debounces input and publishes short/long presses. |
| `app.rs` | Owns UI settings and combines buttons, power status and measurements. Runs without display or GPIO dependencies and is tested on the host. |
| `ui.rs` | Implements bounded setpoint/brightness changes and page transitions. |
| `display.rs` | Owns the OLED/reset pin, draws snapshots, applies brightness and retries initialization or recovery after errors. |
| `pd.rs` / `power.rs` | Own the FUSB302B/PD engine and select an allowed power contract. |
| `control.rs` / `sensors.rs` | Own ADC and the heater-off output; acquire independent sensor scans and convert nominal readings. |
| `heartbeat.rs` | Emits liveness diagnostics independently of acquisition. |

The controller is read-only and holds the heater output low. The setpoint is
UI state only. Future regulation must receive settings and power status
independently of display activity; PWM and heating are not implemented.

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

Buttons, application, controller, display and heartbeat run on the thread executor. The
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

The application/display separation and read-only controller have not yet been
flashed or hardware-tested. See the [acquisition contract](measurements.md).
The earlier hardware results in [PD scheduling](pd-scheduling.md) apply to the
previous image. The next hardware check should include 5 V flashing, cold boot,
button/menu/brightness stress and simultaneous PD/ADC capture at 28 V EPR.

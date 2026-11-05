# PD response deadlines and UI scheduling

The [fusb302b 0.2.0 release](https://crates.io/crates/fusb302b/0.2.0) refactors
the PHY driver. Application scheduling remains an open hardware issue.
The current firmware runs PD, buttons, heartbeat and OLED on one cooperative
thread executor. CPU work without an await delays all other tasks, including
delivery of data that the I2C interrupt handler has already received.

## Hardware evidence, 2026-11-05

The setup was Pinecil V2, Pico JTAG, KM003C PD sniffing and 1 kHz ADC capture,
with the heater disabled. The driver negotiated 5 V, 20 V SPR and 28 V EPR.
A 300-second candidate capture contained three spontaneous voltage drops;
the user had not moved the cables. A separate 240-second capture without
debugger attachment also recorded a reset. Thus debugger attachment is not
required to trigger the fault.

An instrumented run later sustained 28 V for 338 seconds and recorded 864
KeepAlive/Ack pairs, but another run reproduced resets during button use.
One clean capture is insufficient evidence that the intermittent fault is fixed.

The final captured failure with shortened logs had this MCU timeline:

| MCU time, seconds | Operation |
|---|---|
| 88.303147–88.304031 | Write KeepAlive to the FUSB302 TX FIFO |
| 88.310193–88.322591 | Draw an OLED frame without yielding |
| 88.329724–88.333045 | Read the hardware retry engine's GoodCRC from RX FIFO |
| 88.337732–88.338948 | Next RX FIFO read is cancelled |
| 88.364071 | Policy reports Hard Reset; source then cuts VBUS |

The sniffer recorded the source's KeepAliveAck and the FUSB302's automatic
GoodCRC response. The policy engine did not receive the queued message before
its response deadline. Bytes in a cancelled I2C trace are not valid received
data: the caller's buffer may never have been filled.

Profiling on stable 5 V measured approximately 42 us for the GPIO snapshot,
4–5 ms for the old verbose heartbeat formatting, 2.3 ms for the short heartbeat,
and 9–13 ms for framebuffer drawing. These are elapsed times from the MCU's
nominal RC32M clock, not externally calibrated cycle measurements. OLED
I2C FIFO errors were also observed and recovered; IRQ latency still needs review.

## Current changes and remaining work

- Keep routine button/UI diagnostics at debug level and use a short heartbeat.
- Format `log` messages outside RTT's critical section.
- Yield between OLED I2C transactions, allowing a waiting PD task to acquire
  the shared bus.
- Split framebuffer preparation, header and footer drawing with cooperative
  yields. This last change has passed offline checks; its hardware validation
  is pending.
- Preserve binary diagnostics so timing can be checked without formatting
  packet dumps on the PD response path.

An interrupt executor for PD is the next architectural step. It must use the
BL706's CLIC/software interrupt route, keep polling bounded, and preserve the
existing FPU context. The I2C ISR must remain able to service its two-word
hardware FIFO promptly; CPU priority does not eliminate shared-bus contention
or long interrupt-disabled sections. The present HAL has not implemented this
executor yet.

## Diagnostic records

Build with `--features pd-diagnostics`. RTT channel 0 is text; channel 1 carries
44-byte little-endian records: start/end timestamps, metadata, and 32 data bytes.
Both channels are nonblocking. A full trace channel drops a complete record.
The source is [pd_trace.rs](../src/pd_trace.rs); the host reader is
[pd_diagnostics.py](../tools/pd_diagnostics.py).

For I2C records, metadata contains address, register, direction, length, error
and cancellation flags. Only successful FIFO operations and failed/cancelled
operations are streamed. Data is truncated to 32 bytes and is unreliable on
an error or cancellation. The bounded RAM trace additionally retains the first
256 I2C operations; it is not a history of the entire session.

CPU records use address zero and the register field as a stage identifier:

| Stage | Measurement |
|---|---|
| 1 | Whole heartbeat |
| 2 | Legacy GPIO snapshot |
| 3 | Legacy verbose heartbeat formatting |
| 4 | Whole framebuffer render, including yields |
| 6 | Clear buffer and prepare strings |
| 7 | Draw header |
| 8 | Draw footer and animation marker |

For the legacy heartbeat comparison, use `--verbose-heartbeat 1` on stable
5 V only and return to `--verbose-heartbeat 0` afterwards. Normal builds do
not contain the legacy heartbeat branch. Captures stop on target power loss;
record PD and voltage independently to retain the reset sequence.

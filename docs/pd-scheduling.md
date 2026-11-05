# PD response deadlines and UI scheduling

The [fusb302b 0.2.0 release](https://crates.io/crates/fusb302b/0.2.0) refactors
the PHY driver. Application scheduling remains an open hardware issue.
The original firmware ran PD, buttons, heartbeat and OLED on one cooperative
thread executor. CPU work without an await delayed all other tasks, including
delivery of data that the I2C interrupt handler had already received. The current
candidate moves PD to the HAL's machine software interrupt executor, while
buttons, heartbeat and OLED stay in thread mode.

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

A further test added yields between framebuffer preparation, header and footer.
The longest drawing segment fell to about 7.3 ms, but a button/menu stress test
still caused a reset. VBUS fell below 27 V at capture time 78.714 s and recovered
at 80.768 s, matching the user's observed shutdown. The relevant MCU sequence:

| MCU time, seconds | Operation |
|---|---|
| 58.222972–58.223818 | Write KeepAlive |
| 58.233235–58.234846 | Read GoodCRC |
| 58.239686–58.246529 | Draw header |
| 58.250565–58.253278 | Read KeepAliveAck header and payload |
| 58.254037–58.262650 | Cancel its four-byte CRC tail read |
| 58.283683 | Report Hard Reset |

The source's Ack and the PHY's automatic GoodCRC were present on the wire.
There was no successful completion of the message read before the policy's
response timeout. Cooperative yields alone did not resolve the issue.

## Current changes and remaining work

- Keep routine button/UI diagnostics at debug level and use a short heartbeat.
- Format `log` messages outside RTT's critical section.
- Yield between OLED I2C transactions, allowing a waiting PD task to acquire
  the shared bus.
- Split framebuffer preparation, header and footer drawing with cooperative
  yields. This improves scheduling opportunities but failed the EPR stress test.
- Preserve binary diagnostics so timing can be checked without formatting
  packet dumps on the PD response path.

PD now runs on the HAL's
[machine software interrupt executor](https://github.com/okhsunrog/bl706-embassy/blob/75b485167197a9d5aeb53e1d64936b861889aa08/docs/executor.md).
The port passed its separate hardware test for preemption, timer/I2C wakeups,
thread-to-IRQ mutex unlock and interrupted FPU context. A first non-preemptible port caused OLED FIFO errors at
5 V. The HAL now gives peripheral ISRs a higher CLIC level than the software
executor and preserves nested return/FP context. PD polls should still stay
short; CPU priority does not eliminate shared-bus contention or long
interrupt-disabled sections.

The preemptible diagnostic build sustained EPR throughout a 420.067-second
PD/ADC capture, including button/menu stress. After crossing 27 V at capture
time 20.450 s, no sampled voltage fell below 27 V. The 360-second RTT/binary
recording contained 958 KeepAlive/Ack pairs without a restart, FIFO error or
cancelled FIFO read. Successful seven-byte FIFO reads took at most 2.407 ms;
four-byte tails took at most 1.680 ms. The longest PD poll was 23.959 ms,
including preemption; per-poll trace serialization happens after that measurement.

One OLED FIFO error at MCU time 93.662 s triggered display recovery without
losing EPR. Its exact cause remains to be isolated; the executor does
not eliminate every FIFO fault. ADC recorded 419,434 samples with 636 missing
samples in six gaps (largest 408 ms), so it is not a gap-free voltage trace.
Continuous RTT and KeepAlive/Ack traffic provide independent restart evidence.

The normal build (without either diagnostic feature) was then programmed into
flash with readback verification and cold-booted from the EPR source. Its
420.070-second capture included 274.974 seconds above 27 V, including button
and brightness-menu stress. There were 716 KeepAlive/Ack pairs and no voltage
drop, Soft Reset or Hard Reset after the EPR transition. The final 162.451
seconds of this capture ran with OpenOCD shut down and no debug session.
ADC captured 420,049 samples; its only 24-sample gap preceded EPR attachment.
The user confirmed normal display/buttons and no blanking or shutdown.

RTT read before and after the passive interval contained consecutive heartbeat
ticks 0 through 296, with no OLED error, panic or restart for the recorded EPR
test. After the voltage capture had finished, the unread 4 KiB nonblocking RTT
buffer filled: ticks 297–329 were dropped before the host resumed draining it.
Ticks then continued at 330–344. This host-read gap is distinct from a restart.
These bounded tests validate the new scheduling on this setup; they do not
establish that every earlier intermittent I2C fault has been eliminated.

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
| 9 | One PD task poll in the software interrupt, before trace serialization |

For the legacy heartbeat comparison, use `--verbose-heartbeat 1` on stable
5 V only and return to `--verbose-heartbeat 0` afterwards. Normal builds do
not contain the legacy heartbeat branch. Captures stop on target power loss;
record PD and voltage independently to retain the reset sequence.

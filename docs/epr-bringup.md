# EPR and I2C bring-up, 2026-11-05

The default XIP/IMFC firmware automatically negotiates fixed 20 V (or 5 V),
advertises EPR capability when the source supports it, enters EPR with an 84 W
operational PDP and requests the source's fixed 28 V profile, limited to 3 A.
The EPR request contains the original PDO and its actual object position.
36/48 V and PPS/AVS profiles are not selected. Contracts are displayed only
after Accept and PS_RDY. The heater remains OFF throughout.

The policy follows the pinned usbpd 3.1.0
[EPR sink example](https://github.com/elagil/usbpd/blob/b1f0bb9720e362039ac5e5a511199c828da26c04/examples/embassy-nucleo-g474re-sink-epr/src/power.rs).
Twelve application host tests cover the UI, SPR fallback, EPR object position,
PDO copying, current limits and refusal of higher-voltage profiles.

## Hardware result

Pinecil V2 BL706, FUSB302 Device ID 0x81, Pico Grapple CMSIS-DAP at 1 MHz JTAG,
KM003C serial 007965, firmware 2.0.7. Nominal I2C is 300 kHz; its physical clock
frequency has not been measured. No UART or heater load was used.

The final default firmware reported 20 V at target uptime 391679 us and
28 V at 885292 us. Wire traffic independently contains:

- SPR Request, Accept and PS_RDY;
- EPR Enter with PDP 84, EnterAcknowledged and EnterSucceeded;
- both chunks of EPR_Source_Capabilities and the sink's chunk request;
- EPR_Request `89 26 2c b1 44 81 f4 c1 18 00`: PDO8, 28 V, 3 A, exact PDO copy;
- Accept, PS_RDY and continuing EPR KeepAlive/KeepAliveAck exchanges.

A settled 180-second window contains 180,000 consecutive ADC samples with no
sequence gaps. VBUS ranged from 27.892181 to 27.910736 V (median 27.901165 V).
There were 472 KeepAlive and 472 Ack messages in the corresponding host-receipt
window, with maximum KeepAlive spacing about 402 ms. ADC and PD placement on the
plot uses host USB-batch receipt times; this is not precision synchronization
between the meter's ADC and PD clocks. PD-to-PD deltas use meter timestamps.

The RTT capture contains continuing Embassy heartbeats and no OLED I2C errors,
panic or repeating PD resets. The user confirmed the 28 V display, animation,
buttons and menu worked without corruption. This is an idle negotiation test;
the configured 3 A ceiling is not a measured 3 A load.

Two further starts were tested by halting the core, allowing the source to
remove/reset VBUS when KeepAlive stopped, and reattaching after power returned.
Both boots automatically established 20 V then 28 V, at target uptimes
392655/886084 us and 391353/884294 us respectively. No button or manual PD start
command was used. These are source-driven VBUS resets, not cable-orientation or
all-charger compatibility tests.

## Timing and cancellation findings

The earlier diagnostic builds could acknowledge incoming PD packets in hardware
but respond too late in software. Detailed synchronous RTT formatting in policy
callbacks and the PHY's extra I2C transactions consumed the response window.
One captured EPR Request appeared 29 ms after the final capabilities chunk,
alongside a source reset marker. The reduced-transaction diagnostic version
sent it after 22 ms and established 28 V. These observations do not assign one
cause to every earlier reset variant.

In the final default build, without the RAM trace, the same interval measured
14 ms on the first start and 13 ms on both subsequent source-driven restarts.

Driver commit `17b0d6b` enables AUTO_CRC after the final initialization reset,
reads short control frames in one FIFO transaction, and keeps POWER=0x0f during
the active session, avoiding four extra POWER transactions per transmit. Fourteen
emulated-PHY tests pass, including back-to-back minimum/maximum frames and
initialization with a packet arriving as AUTO_CRC is enabled.

Detailed PDO/request logs are debug-level. Policy callbacks do not wake the
OLED before sending a Request; status is published after the contract completes.
The OLED adapter reports the underlying I2C error, rather than only the display
crate's generic BusWriteError.

A separate hardware test reproduced HAL cancellation failures. It raced a FUSB
Device ID read against seven timer delays, 20 attempts each, then immediately
read Device ID again. Before the fix: 58 cancellations, 39 follow-up BusBusy
errors. HAL commit `27de879` waits asynchronously, with the packet timeout, for
the controller BUS_BUSY flag to clear before starting another packet. After:
48 actual cancellations, all 140 follow-up reads correct, no errors. Timer/
transfer phase explains the differing cancellation counts. This does not prove
arbitrary cancellation midway through a slave FIFO read preserves its stream
position, nor does it implement recovery from physically stuck SDA/SCL.

## IronOS comparison

The reviewed IronOS revision is `c7413e0`, using usb-pd submodule `846c3ae`.
Its Pinecil V2 setup selects 300 kHz, SCL synchronization and one-cycle deglitch.
Its I2C wrapper services FIFOs in the ISR, serializes users with a semaphore and
disables the BLE interrupt during transfers. Our HAL uses the matching clock/
filter settings, ISR FIFO service and shared-bus serialization. BLE is not
running in this firmware.

`BSP.cpp::unstick_I2C()` has a commented-out body in this revision. The SDK's
"this sucks as a workaround" TODO is in a blocking zero-data/address special
case, not the normal interrupt-driven FUSB transfer path. Neither is evidence
of active recovery for our cancellation failure. We reproduced that failure
directly and fixed the transient BUS_BUSY handling in our HAL.

## Reproducing diagnostics

`cargo run -r --features pd-manual-start` produces a diagnostic image that waits
for the debugger before starting PD. After programming verification, stop that
runner to release the probe, then run:

```sh
openocd -f tools/pinecil-v2.cfg -c 'riscv set_mem_access sysbus progbuf; init'
```

In another terminal:

```sh
# Cancellation regression; then wait again for a start command.
uv run --no-project tools/pd_diagnostics.py --start-pd --command 2 --seconds 5
# Start PD and save the bounded RAM I2C trace while draining RTT.
uv run --no-project tools/pd_diagnostics.py --start-pd --seconds 10 --trace-output trace.json
```

The trace uses fixed-size atomic records and no RTT formatting in I2C calls.
It records completed/error/cancelled operations and stops retaining new records
after the first 256. The tool incrementally copies records before a power reset
can erase RAM. Manual waiting can outlast a source's capabilities advertisements;
the PE may need a Hard Reset and a new attach window before negotiation.

The default build includes neither the manual start gate nor RAM trace. The
tool can also drain its RTT without `--start-pd`.

## Checks and remaining scope

Application checks, clippy (default and diagnostic), XIP/IMFC ELF audit and five
boot-image tests pass. The HAL's full offline suite and the driver's log/defmt
clippy checks pass. Artifacts include the flashed ELF, prepared boot image,
raw PD/ADC captures, RTT logs, cancellation before/after logs and SHA-256 sums.

The probe-rs runner can lose its RTT connection when a PD source power reset
occurs after flashing. Reattaching to the running image works. This transport
limitation is separate from automatic boot and PD negotiation.

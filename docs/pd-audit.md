# Pinecil V2 PD integration audit

Source comparison on 2026-11-04:

- [Production schematic, ver. 2.0, 2022-06-08](https://files.pine64.org/doc/Pinecil/Pinecil_schematic_v2.0_20220608.pdf).
- [IronOS BSP](https://github.com/Ralim/IronOS/tree/c7413e0d2adfd43156510d4ccbd109cf352ca335/source/Core/BSP/Pinecilv2),
  including `Pins.h`, `Setup.cpp`, `IRQ.cpp`, `configuration.h` and `fusb_user.cpp`.
  This was also the remote `dev` head when checked.
- [IronOS PD adapter](https://github.com/Ralim/IronOS/blob/c7413e0d2adfd43156510d4ccbd109cf352ca335/source/Core/Drivers/USBPD.cpp)
  and `POWThread.cpp`; its pinned
  [usb-pd submodule](https://github.com/Ralim/usb-pd/tree/846c3aeeaf1d9283928540b39ece968d3c344d53).
- [FUSB302B datasheet, rev. 5](https://www.onsemi.com/pdf/datasheet/fusb302b-d.pdf),
  particularly automatic receive/retry and register tables 35-40.
- Application dependency `usbpd` 3.1.0 at `b1f0bb9`, and local `fusb302b`.

## Board connections and initialization

| Item | Board / IronOS evidence | Rust integration |
|---|---|---|
| FUSB I2C address | IronOS uses 8-bit address `0x44` | Correct 7-bit embedded-hal address `0x22` |
| I2C | U1 SCL/SDA share the OLED bus; AF6 on GPIO10/11 | HAL uses GPIO10=SCL, GPIO11=SDA according to peripheral routing. IronOS `Pins.h` labels these backwards but configures AF6 on both, so those names do not change the mux. |
| Rate | IronOS requests 300 kHz, SCL sync, one-cycle deglitch | Same requested configuration; effective frequency has not been measured. |
| INT_N | U1 pin 5, PD_IRQ to GPIO16, R37 10 kohm pull-up | Pull-up input, active-low async wait |
| CC | Both Rd enabled; select one measured CC for BMC | Same `SWITCHES0=0x07/0x0b`, `SWITCHES1=0x25/0x26`; no VCONN drive enabled |
| VBUS sense | U1 pin 2 is NC; `NEEDS_VBUS_PROBE=0` | Must use existing `init_with_vbus_and_interrupt(..., external_detector, ...)`, not an internal-VBUS constructor |
| Heater | Tip_Control GPIO21 | Held low; no heating or temperature control |
| OLED/buttons | Reset GPIO3, keys GPIO25/28, active high with pull-downs | Same routing; prior user acceptance of working UI/buttons |

The driver already supported external VBUS detection. Selecting its internal
detector in this application was an integration error. `()` is the existing
immediate-return detector; it does not measure voltage or detect cable removal.
The board's VIN ADC path is separate and is not implemented in this application.

IronOS and this driver both reset the PHY, power its blocks, enable interrupts,
configure hardware retries, select CC and reset PD logic. Differences that are
not established causes of the observed failure: IronOS polls CC repeatedly,
while the current Rust driver compares two samples; IronOS sets three retries,
while this driver sets two; both initially configure the automatic GoodCRC
revision as PD 2.0. No claim is made that these differences are equivalent for
every charger, orientation, or EPR transition.

## Confirmed driver defects and regression coverage

1. All interrupt sources were unmasked, but `INTERRUPTB` was never read.
   `I_GCRCSENT` is read-to-clear and could keep INT_N asserted. IronOS reads
   the complete status/interrupt block. The driver now services all three
   interrupt registers in a single seven-byte burst and retains observed
   completion/reset flags; RX availability uses the snapshot's `STATUS1.RX_EMPTY`.
2. `HAS_AUTO_RETRY` tells usbpd that hardware already verified GoodCRC.
   FUSB302 still stores that acknowledgement in RX FIFO. Returning it to
   usbpd's Ready policy state caused `NotSupported -> GoodCRC` repeatedly.
   The driver now drains GoodCRC without forwarding it, preserving subsequent
   policy messages. IronOS consumes acknowledgements in its send state.
3. RX/TX/hard-reset loops checked their deadline before awaiting a GPIO event.
   A silent high INT_N could make this wait infinite. The wait now races its
   deadline, including when no IRQ ever arrives.
4. Stale TX completion could acknowledge a later transmission, and simultaneous
   TX completion could hide Hard Reset. Stale completions are cleared before
   transmission; Hard Reset takes priority and flushes stale FIFOs/PD state.
5. An oversized public transmit buffer could panic while assembling FIFO
   tokens. Standard frame bounds are checked before copying.

The first eight emulated-PHY tests run against the unchanged driver `c633aec`
produced seven failures and one pass. The fixed driver passes eleven tests,
including cancellation during reset cleanup, an otherwise idle asserted
`I_GCRCSENT`, and the maximum 30-byte standard frame. These are
host regressions, not a substitute for hardware validation.

The first corrected driver used separate IRQ register transactions and missed
the laptop's response deadline: Request appeared about 27 ms after capabilities.
The final version reads the same seven-byte status/IRQ block as IronOS and uses
two FIFO reads per packet (token/header, then payload/CRC). The laptop accepted
the subsequent Request, followed by PS_RDY, and the repeated acknowledgement
loop stopped. Cancellation tests do not prove arbitrary cancellation halfway
through an RX FIFO I2C transfer is lossless.

## Hardware evidence and limits

KM003C serial 007965, hardware 2.1, firmware 2.0.7; Grapple Probe Pico at 1 MHz
JTAG; heater tip removed. Capture used passive PD monitoring and ADC readings.

Before the external-detector application fix, the charger repeatedly offered
5/9/12/15/20 V and PPS, but the application did not issue a Request. VBUS cycled.
After the fix, with the laptop, the wire trace contained a PDO1 Request at
416.325 s, Accept at 416.328 s, and PS_RDY at 416.364 s. RTT independently
reported a 5000 mV, 3000 mA explicit contract. The laptop offered only 5 V.
The subsequent NotSupported/GoodCRC loop is the acknowledgement defect above;
it must not be described as absence of a contract or unstable 5 V supply.

The combined CLI was validated at 1000 SPS: 14,990 consecutive samples over
14.989 s, no sequence gaps, VBUS 5.141986-5.151751 V, with simultaneous PD
traffic. A separate Ctrl+C capture preserved 2,655 samples and stopped both
streams normally.

With the final driver, a cold connection to the PD charger without buttons or
a debugger reset requested PDO5 (20 V, limited to 3 A) at meter time 1593.039 s.
The source sent Accept at 1593.040 s and PS_RDY at 1593.243 s. RTT independently
reported `PD contract ready: 20000 mV 3000 mA (PS_RDY)` and continuing heartbeat.
No repeating NotSupported/GoodCRC stream followed. The user confirmed the
display and buttons worked after this cold start. The capture continued for
about three minutes after negotiation. The 180,849 settled samples ranged from
20.176947 to 20.187494 V. The full capture contains 201,838 accepted samples,
16 rejected sequence samples and two continuity restarts; it is not claimed
to be entirely gap-free despite a zero detected forward-gap count. The
transient during switching/ramping is separate from the settled range; the
raw CSV and summary preserve both.

A remaining runner limitation is distinct from this cold-start result:
after programming/resetting while powered from a PD source, a source power
reset can break the original `probe-rs run` connection. A subsequent RTT monitor
attached to the running image successfully. Automatic reconnect after that
reset is not yet implemented; a fully uninterrupted flash-and-RTT workflow
over arbitrary PD source resets is not claimed.

The preceding results describe the 2026-11-04 SPR policy. The EPR library feature
remains enabled. On 2026-11-05 the DPM was extended to enter EPR and request
fixed 28 V, retaining the 3 A ceiling and SPR fallback. Hardware results and the
additional timing/cancellation fixes are recorded in [epr-bringup.md](epr-bringup.md).

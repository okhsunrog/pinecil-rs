# Runtime EPR exit and SPR programming

Hardware validation on 2026-11-06 used a Pinecil V2, a Raspberry Pi Pico running
the Grapple JTAG port, an EPR charger and a KM003C running the custom passive
PD/ADC monitor. Heating remained **OFF** throughout this test.

The user initially connected power while holding `−`. After that initial
bootstrap, all three flash downloads, the EPR exit and the final BLE check ran
without further cable changes or button presses.

## Exit from EPR

The normal EPR build first reached a confirmed 28 V / 5 A contract. Sending
`pd spr\n` through RTT down-channel 0 produced this sequence:

1. EPR_Request for the source's 15 V / 3 A SPR PDO, still using EPR framing.
2. PS_RDY confirming that power transition.
3. EPR_Mode(Exit).
4. An ordinary SPR Request for 15 V / 3 A, followed by PS_RDY.
5. `PD SPR ready: EPR inactive` and `spr_ready=true` in RTT.

The independent KM003C trace measured **429.413 ms** from the first
EPR_Request to the final SPR PS_RDY. The window from two seconds before that
request to ten seconds after it contains 554 ADC records, with VBUS between
14.976 and 27.907 V. It contains **no Hard Reset or Cable Reset** and no return
to 5 V. RTT shows continuous firmware uptime and heating OFF.

The immediate command acknowledgement now says `negotiation pending`.
Automation must match the full `PD SPR ready: EPR inactive` message, or inspect
`spr_ready=true`; an acknowledgement does not authorize halting the CPU.

## Flash programming and final state

Every download below used `probe-rs download --verify`, followed by reset:

| Image | Supply during programming | Download time |
| --- | --- | --- |
| Initial `pd-spr-only` build | 5 V | 96.87 s |
| Normal EPR build | Confirmed 15 V SPR | 96.83 s |
| Final `pd-spr-only` build | 15 V SPR after runtime EPR exit | 96.65 s |

The final installed image has `ble`, `fpu` and `pd-spr-only` enabled. It reports
15 V / 3 A, `spr_requested=true`, `spr_ready=true`, valid sensor measurements,
zero ADC errors and heating OFF. Its final 20-second meter window contains
889 records between 14.976947 and 14.984564 V.

A laptop BLE test passed one connect/read/write/notify/disconnect cycle,
including eight consecutive notifications. This test did not request heating.

Default builds still support EPR. A runtime `pd spr` preference lasts until
reset; the `pd-spr-only` build feature keeps SPR preference across boots.
The Cargo runner does not yet issue `pd spr` automatically. Before programming
a normal EPR build, request SPR and wait for the explicit readiness result.

## Remaining boundary

Startup after reset was **not** free of PD recovery: the full captures contain
one Hard Reset ordered set around each startup before the contract settles,
including the final SPR startup. The initiating device and cause have not been
isolated. The reset-free result above applies to runtime EPR exit and steady
SPR programming, not to the entire boot sequence.

This test did not exercise the KM003C's active sink/profile-switching commands.
Its role here was passive monitoring; the Pinecil FUSB302 owned PD negotiation.

## Evidence

- [Machine-readable summary and image hashes](pd-runtime-hardware.json).
- [Raw PD/ADC exit window](pd-runtime-exit.ndjson).
- [RTT exit log](pd-runtime-exit-rtt.txt).
- [Final BLE test](pd-runtime-ble.txt).

The tested policy source is
[`da13e97`](https://github.com/okhsunrog/pinecil-rs/commit/da13e97fe145241608faf6520a12603cc2f37c2a).
The final image additionally contains the command acknowledgement wording
change described above; the flash payload hashes identify the exact images.

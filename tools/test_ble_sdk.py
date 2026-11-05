# /// script
# requires-python = ">=3.13"
# dependencies = ["bleak==3.0.2"]
# ///
"""Radio reference test for the heater-off C SDK fixture, not the Rust GATT service."""
import asyncio
from bleak import BleakClient, BleakScanner


async def main():
    device = await BleakScanner.find_device_by_filter(
        lambda device, advertisement: advertisement.local_name == "BL702-BLE-DEV",
        timeout=15,
    )
    assert device, "SDK advertisement not observed"
    print("Found SDK", device, flush=True)
    for cycle in range(10):
        async with BleakClient(device, timeout=15) as client:
            for sample in range(6):
                name = await client.read_gatt_char("2a00")
                appearance = await client.read_gatt_char("2a01")
                assert name == b"BL702-BLE-DEV", name
                assert len(appearance) == 2, appearance
                await asyncio.sleep(0.5)
            print("Verified 12 reads", cycle, flush=True)
        print("Disconnected", cycle, flush=True)
        await asyncio.sleep(2)
    print("PASS: 10 SDK connect/read/disconnect cycles, 120 ATT reads", flush=True)


if __name__ == "__main__":
    asyncio.run(main())

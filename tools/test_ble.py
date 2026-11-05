# /// script
# requires-python = ">=3.13"
# dependencies = [
#     "bleak==3.0.2",
# ]
# ///
"""Physical GATT smoke/soak test for the heater-off BLE example."""
import argparse
import asyncio

from bleak import BleakClient, BleakScanner

CHARACTERISTIC = "b1706001-0000-4000-8000-000000000001"


async def main(cycles, seconds):
    device = await BleakScanner.find_device_by_filter(
        lambda device, advertisement: advertisement.local_name == "Pinecil Rust",
        timeout=15,
    )
    assert device, "Pinecil advertising was not observed"
    print("Found", device, flush=True)
    for cycle in range(cycles):
        async with BleakClient(device, timeout=15) as client:
            print("Connected", cycle, flush=True)
            for service in client.services:
                print("Service", service.uuid, flush=True)
            value = await client.read_gatt_char(CHARACTERISTIC)
            assert len(value) == 4, value
            print("Read", int.from_bytes(value, "little"), flush=True)
            requested = 1000 + cycle * 100
            await client.write_gatt_char(
                CHARACTERISTIC, requested.to_bytes(4, "little"), response=True
            )
            value = int.from_bytes(await client.read_gatt_char(CHARACTERISTIC), "little")
            assert requested <= value <= requested + 1, (requested, value)
            print("Write/read verified", value, flush=True)
            notifications = []

            def received(sender, data):
                assert len(data) == 4, data
                value = int.from_bytes(data, "little")
                notifications.append(value)
                print("Notification", value, flush=True)

            await client.start_notify(CHARACTERISTIC, received)
            await asyncio.sleep(seconds)
            await client.stop_notify(CHARACTERISTIC)
            assert len(notifications) >= max(1, seconds - 3), notifications
            assert all(b == a + 1 for a, b in zip(notifications, notifications[1:])), notifications
        print("Disconnected", cycle, flush=True)
        await asyncio.sleep(2)
    print(f"PASS: {cycles} connect/read/write/notify/disconnect cycles", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cycles", type=int, default=3)
    parser.add_argument("--seconds", type=int, default=8)
    args = parser.parse_args()
    if args.cycles < 1 or args.seconds < 4:
        parser.error("cycles must be positive; seconds must be at least four")
    asyncio.run(main(args.cycles, args.seconds))

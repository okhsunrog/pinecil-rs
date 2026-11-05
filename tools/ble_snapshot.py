"""Freeze and save the BLE flight recorder; MMIO uses halted PROGBUF only."""
import argparse
import json
import socket
import subprocess

p = argparse.ArgumentParser(description=__doc__)
p.add_argument("elf")
p.add_argument("output")
a = p.parse_args()
symbols = {}
for line in subprocess.check_output(["readelf", "-sW", a.elf], text=True).splitlines():
    f = line.split()
    if len(f) >= 8 and f[0].endswith(":") and f[0][:-1].isdigit():
        symbols[f[-1]] = int(f[1], 16)
s = socket.create_connection(("127.0.0.1", 6666), 3)
s.settimeout(10)


def cmd(command):
    s.sendall(command.encode() + b"\x1a")
    data = b""
    while not data.endswith(b"\x1a"):
        part = s.recv(65536)
        if not part:
            raise EOFError("OpenOCD disconnected")
        data += part
    return data[:-1].decode()


def words(address, count):
    raw = cmd(f"read_memory {address:#x} 32 {count}")
    result = [int(x, 0) for x in raw.split()]
    assert len(result) == count, raw
    return result


base = symbols["BLE_FLIGHT_RECORDER"]
assert any(lo <= base and base + 32 + 240 * 32 <= hi for lo, hi in (
    (0x22022000, 0x22024000),  # Original standalone investigation images.
    (0x2202a000, 0x2202e000),  # Current stack/diagnostic partition.
))
cmd(f"write_memory {base + 8:#x} 32 {{1}}")
header = words(base, 8)
records = []
for n in range(max(0, header[1] - 240), header[1]):
    records.append([n, *words(base + 32 + (n % 240) * 32, 8)])
result = {"header": header, "records": records, "ram": {}, "mmio": {}}
for name, count in (("ea_env", 8), ("rwip_env", 4), ("lld_evt_env", 11), ("ble_ke_env", 9), ("ble_ke_event_env", 12)):
    address = symbols[name]
    result["ram"][name] = {"address": address, "words": words(address, count)}
cmd("halt")
try:
    result["registers"] = {name: cmd(f"reg {name}") for name in ("pc", "ra", "sp", "mstatus", "mcause", "mepc", "mtval")}
    cmd("riscv set_mem_access progbuf")
    for address in (0x28000000, 0x2800000C, 0x28000010, 0x28000014, 0x2800001C, 0x28000020, 0x28000030, 0x280000F8):
        result["mmio"][hex(address)] = words(address, 1)[0]
    for address in (0x02800048, 0x02800448):
        result["mmio"][hex(address)] = cmd(f"read_memory {address:#x} 8 1")
finally:
    cmd("riscv set_mem_access sysbus progbuf")
    cmd("resume")
with open(a.output, "w") as f:
    json.dump(result, f, indent=2)
print(json.dumps({k: v for k, v in result.items() if k != "records"}, indent=2))

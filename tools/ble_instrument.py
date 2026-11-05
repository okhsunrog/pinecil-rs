"""Instrument one audited timer-store site in a diagnostic ELF, never the vendor archive.

The selected controller's ea_prog_timer saves ra at entry and restores it on
every return. At +0xd2 it executes LUI a5,0x28000; SW s2,0xf8(a5).
Replace those eight bytes with a call preserving all registers used afterwards.
The helper samples the store in ITCM. A RAM control word can intentionally
delay one near-deadline store for fault injection; leave it zero otherwise.
"""
from pathlib import Path
import struct
import subprocess
import sys

source, output = map(Path, sys.argv[1:])
data = bytearray(source.read_bytes())
symbols = {}
sizes = {}
for line in subprocess.check_output(["readelf", "-sW", source], text=True).splitlines():
    f = line.split()
    if len(f) >= 8 and f[0].endswith(":") and f[0][:-1].isdigit():
        symbols[f[-1]] = int(f[1], 16)
        sizes[f[-1]] = int(f[2], 0)
function = symbols["ea_prog_timer"]
function_size = sizes["ea_prog_timer"]
target = symbols["ble_trace_timer_store"]
assert 0x22014000 <= target < 0x2201B800, hex(target)
phoff = struct.unpack_from("<I", data, 28)[0]
phsize, count = struct.unpack_from("<HH", data, 42)
for i in range(count):
    kind, offset, vma, _, size, *_ = struct.unpack_from("<8I", data, phoff + i * phsize)
    if kind == 1 and vma <= function < function + function_size <= vma + size:
        start = offset + function - vma
        # Linker relaxation shortens calls preceding the store. Match the exact
        # instructions inside the audited function instead of a relaxed offset.
        pattern = bytes.fromhex("b707002823ac270f")
        pos = data.find(pattern, start, start + function_size)
        assert pos >= 0 and data.find(pattern, pos + 1, start + function_size) < 0
        site = vma + pos - offset
        delta = target - site
        upper = (delta + 0x800) >> 12
        lower = delta - (upper << 12)
        # AUIPC ra, upper; JALR ra, ra, lower. The native frame owns saved ra.
        struct.pack_into("<II", data, pos, ((upper & 0xFFFFF) << 12) | 0x97,
                         ((lower & 0xFFF) << 20) | 0x80E7)
        break
else:
    raise ValueError("timer site has no file-backed load segment")
output.write_bytes(data)
print(f"Instrumented timer store {site:#x} -> {target:#x}: {output}")

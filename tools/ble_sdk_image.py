"""Package the SDK fixture for direct ROM boot, preserving its runtime addresses.

Add the physical 4 KiB offset and a real .bootheader section plus load segment.
probe-rs discovers sections: a program-header-only boot header is insufficient.
"""
import argparse
from pathlib import Path
import struct

from flash_image import load_segments, write_prepared

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('source', type=Path)
p.add_argument('output', type=Path)
a = p.parse_args()
data = bytearray(a.source.read_bytes())
load_segments(data)  # Validate ELF32/RISC-V and table bounds before modification.
phoff = struct.unpack_from('<I', data, 28)[0]
shoff = struct.unpack_from('<I', data, 32)[0]
phsize, phnum, shsize, shnum, strindex = struct.unpack_from('<5H', data, 42)
assert phsize == 32 and shsize == 40 and strindex < shnum
sections = [list(struct.unpack_from('<10I', data, shoff+i*shsize)) for i in range(shnum)]
loads = []
for i in range(phnum):
    pos = phoff+i*phsize
    kind, offset, vma, paddr, filesz, memsz, flags, align = struct.unpack_from('<8I', data, pos)
    if kind == 1 and filesz:
        assert 0x23000000 <= paddr < paddr+filesz <= 0x230ff000
        loads.append(offset)
        struct.pack_into('<I', data, pos+12, paddr+0x1000)
assert phoff+(phnum+1)*phsize <= min(loads), 'no room for one load segment'
strings = sections[strindex]
name_offset = strings[5]
new_strings = data[strings[4]:strings[4]+strings[5]] + b'.bootheader\0'
strings[4] = len(data)
strings[5] = len(new_strings)
data.extend(new_strings)
data.extend(bytes((-len(data)) % 4))
header_offset = len(data)
data.extend(bytes(176))
struct.pack_into('<8I', data, phoff+phnum*phsize, 1, header_offset,
                 0x21000000, 0x23000000, 176, 176, 4, 4)
sections.append([name_offset, 1, 2, 0x21000000, header_offset, 176, 0, 0, 4, 0])
struct.pack_into('<I', data, 32, len(data))
for section in sections:
    data.extend(struct.pack('<10I', *section))
struct.pack_into('<H', data, 44, phnum+1)
struct.pack_into('<H', data, 48, shnum+1)
a.output.write_bytes(data)
print(write_prepared(a.output))

"""Prepare BL702-format, unsigned Pinecil V2 flash ELFs with CRC32 and SHA256.

References: Bouffalo bflb-mcu-tool libs/bl702/{bootheader_cfg_keys,img_create_do}.py.
The linked ELF uses physical flash load addresses and retains RAM debug symbols.
"""
from hashlib import sha256
from pathlib import Path
import struct
import zlib

BASE = 0x23000000
CAPACITY = 1024 * 1024
IMAGE_OFFSET = 4096
HEADER_SIZE = 176


def load_segments(elf):
    if elf[:7] != b"\x7fELF\x01\x01\x01" or len(elf) < 52:
        raise ValueError("expected ELF32 little endian")
    if struct.unpack_from("<H", elf, 18)[0] != 243:
        raise ValueError("expected RISC-V ELF")
    phoff = struct.unpack_from("<I", elf, 28)[0]
    shoff = struct.unpack_from("<I", elf, 32)[0]
    shsize, shcount = struct.unpack_from("<HH", elf, 46)
    if shsize != 40 or shcount == 0 or shoff + shsize * shcount > len(elf):
        raise ValueError("invalid section header table")
    for i in range(shcount):
        _, kind, _, _, offset, size, *_ = struct.unpack_from("<10I", elf, shoff + i * shsize)
        if kind != 8 and offset + size > len(elf):
            raise ValueError("truncated section")
    entsize, count = struct.unpack_from("<HH", elf, 42)
    if entsize != 32 or phoff + count * entsize > len(elf):
        raise ValueError("invalid program header table")
    segments = []
    for i in range(count):
        kind, offset, vma, paddr, size, memsz, flags, align = struct.unpack_from(
            "<8I", elf, phoff + i * entsize
        )
        if kind == 1 and size:
            if size > memsz or offset + size > len(elf):
                raise ValueError("invalid load segment")
            segments.append((paddr, offset, size, vma))
    return sorted(segments)


def prepare(elf):
    segments = load_segments(elf)
    headers = [s for s in segments if s[0] == BASE and s[2] == HEADER_SIZE]
    if not headers:
        if any(BASE <= p < BASE + CAPACITY for p, _, _, _ in segments):
            raise ValueError("flash ELF has no boot header")
        return None  # Plain RAM build.
    if len(headers) != 1:
        raise ValueError("duplicate boot header")
    payload = [s for s in segments if s not in headers]
    if not payload or payload[0][0] != BASE + IMAGE_OFFSET:
        raise ValueError("missing flash bootstrap at physical offset 0x1000")
    end = max(p + size for p, _, size, _ in payload)
    length = (end - BASE - IMAGE_OFFSET + 15) & ~15
    if length <= 0 or IMAGE_OFFSET + length > CAPACITY:
        raise ValueError("flash image exceeds 1 MiB")
    image = bytearray(b"\xff" * length)
    previous = BASE + IMAGE_OFFSET
    for paddr, offset, size, _ in payload:
        if paddr < previous or paddr + size > end:
            raise ValueError("overlapping or non-flash load segments")
        start = paddr - BASE - IMAGE_OFFSET
        image[start:start + size] = elf[offset:offset + size]
        previous = paddr + size
    entry = struct.unpack_from("<I", elf, 24)[0]
    if entry != BASE:
        raise ValueError("unexpected flash entry point")

    header = bytearray(HEADER_SIZE)
    struct.pack_into("<II", header, 0, 0x504E4642, 1)
    header[8:96] = bytes.fromhex(
        # MX25L8006, reviewed against the SDK Mxic profile and board backup.
        # ioMode bit4 disables cache burst wrap for ordinary 1-1-1 SPI reads.
        "46 43 46 47 10 00 01 01 66 99 ff 03 9f 00 9f 00 "
        "04 c2 00 01 c7 20 52 d8 06 02 32 00 0b 01 0b 01 "
        "3b 01 bb 00 6b 01 eb 02 eb 02 02 50 00 01 00 01 "
        "01 00 02 01 02 01 ab 01 05 00 00 00 01 00 00 00 "
        "38 ff 00 ff 77 03 02 40 77 03 02 f0 2c 01 b0 04 "
        "b0 04 05 00 ff ff 14 00"
    )
    struct.pack_into("<I", header, 96, zlib.crc32(header[12:96]))
    # Start on RC32M with an 8 MHz SPI clock; the RAM runtime sets its own clock.
    header[100:112] = bytes.fromhex("50 43 46 47 02 00 00 00 01 03 00 00")
    struct.pack_into("<I", header, 112, zlib.crc32(header[104:112]))
    # No segments, cache enabled; CRC/hash checks enabled; no boot2 or partitions.
    # BL702 flash boot uses an entry offset relative to the XIP image base.
    struct.pack_into("<4I", header, 116, 0x300, length, entry - BASE, IMAGE_OFFSET)
    header[132:164] = sha256(image).digest()
    struct.pack_into("<I", header, 172, zlib.crc32(header[:172]))
    result = bytearray(elf)
    offset = headers[0][1]
    result[offset:offset + HEADER_SIZE] = header
    return result, image, header


def write_prepared(path):
    prepared = prepare(path.read_bytes())
    if prepared is None:
        return path
    elf, image, header = prepared
    output = path.with_name(path.name + ".flash.elf")
    output.write_bytes(elf)
    print(f"Flash image: {len(image)} bytes at physical offset 0x1000; "
          f"SHA256 {sha256(image).hexdigest()}", flush=True)
    return output


if __name__ == "__main__":
    import sys
    print(write_prepared(Path(sys.argv[1])))

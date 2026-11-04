"""Format/rejection tests using the actual linked flash ELF as a fixture."""
import hashlib
from pathlib import Path
import struct
import sys
import unittest
import zlib
from flash_image import BASE, load_segments, prepare

ELF = Path(sys.argv.pop(1)).read_bytes()


class FlashImageTests(unittest.TestCase):
    def test_rom_integrity_fields(self):
        elf, image, header = prepare(ELF)
        self.assertEqual(len(elf), len(ELF))
        self.assertEqual(header[:4], b"BFNP")
        self.assertEqual(header[12], 0x10, "1-1-1 SPI must disable cache burst wrap")
        self.assertEqual(header[132:164], hashlib.sha256(image).digest())
        for start, end, crc in [(12, 96, 96), (104, 112, 112), (0, 172, 172)]:
            self.assertEqual(zlib.crc32(header[start:end]), struct.unpack_from("<I", header, crc)[0])
        self.assertEqual(struct.unpack_from("<4I", header, 116), (0x300, len(image), 0, 4096))
        self.assertEqual(prepare(elf)[0], elf)

    def test_rejects_truncation(self):
        for count in (0, 7, 51, len(ELF) // 8, len(ELF) - 1):
            with self.subTest(count=count), self.assertRaises(ValueError):
                prepare(ELF[:count])

    def test_rejects_wrong_architecture(self):
        elf = bytearray(ELF)
        struct.pack_into("<H", elf, 18, 40)
        with self.assertRaises(ValueError):
            prepare(elf)

    def test_rejects_invalid_entry(self):
        elf = bytearray(ELF)
        struct.pack_into("<I", elf, 24, BASE + 2)
        with self.assertRaises(ValueError):
            prepare(elf)

    def test_rejects_overlapping_load_segments(self):
        elf = bytearray(ELF)
        phoff = struct.unpack_from("<I", elf, 28)[0]
        count = struct.unpack_from("<H", elf, 44)[0]
        payload = []
        for i in range(count):
            pos = phoff + i * 32
            kind, _, _, pa, size, *_ = struct.unpack_from("<8I", elf, pos)
            if kind == 1 and size and pa > BASE:
                payload.append(pos)
        self.assertGreaterEqual(len(payload), 2)
        first = struct.unpack_from("<I", elf, payload[0] + 12)[0]
        struct.pack_into("<I", elf, payload[1] + 12, first)
        with self.assertRaises(ValueError):
            prepare(elf)


if __name__ == "__main__":
    unittest.main()

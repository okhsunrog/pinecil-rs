"""Offline ELF32 checks; no target execution. uv run --no-project tools/audit_elf.py.

Scan instruction encodings, not just objdump text: unsupported opcodes can be
printed as <unknown> by disassemblers without +a. Also checks compressed FP ops.
"""
from pathlib import Path
import argparse
import struct

ROOT = Path(__file__).resolve().parents[1]

def audit(path, fpu=False, probes=False, ble_placement="itcm"):
    data = path.read_bytes()
    assert data[:7] == b"\x7fELF\x01\x01\x01", path
    h = struct.unpack_from("<16sHHIIIIIHHHHHH", data)
    _, _, machine, _, entry, phoff, shoff, flags, _, phsize, phnum, shsize, shnum, shstrndx = h
    assert machine == 243
    assert flags & 6 == (2 if fpu else 0), (path, "float ABI", flags)
    headers = [struct.unpack_from("<10I", data, shoff+i*shsize) for i in range(shnum)]
    strs = headers[shstrndx]
    strings = data[strs[4]:strs[4]+strs[5]]
    def name(table, offset):
        return table[offset:table.index(b"\0", offset)].decode()
    sections = {name(strings, sh[0]): sh for sh in headers}
    syms = {}
    for sh in headers:
        if sh[1] != 2:
            continue
        names = headers[sh[6]]
        table = data[names[4]:names[4]+names[5]]
        for pos in range(sh[4], sh[4]+sh[5], sh[9]):
            n, value, *_ = struct.unpack_from("<IIIBBH", data, pos)
            syms[name(table, n)] = value
    if path.name == "ble":
        assert 0x22014000 <= syms["__data_start"] <= syms["__bss_end"] <= 0x22020000, "BLE calibration overlaps live RAM"
        assert 0x22020000 <= syms["__stack_bottom"] < syms["__stack_top"] <= 0x22024000
        for hot in ("rwble_isr", "rwip_schedule", "lld_evt_end"):
            lo, hi = (0x22014000, 0x22020000) if ble_placement == "itcm" else (0x23000000, 0x230ff000)
            assert lo <= syms[hot] < hi, (hot, "wrong code placement", ble_placement)
        if "BLE_FLIGHT_RECORDER" in syms:
            trace = sections[".ble_trace"]
            assert trace[1] == 8 and 0x22022000 <= trace[3] < trace[3]+trace[5] <= 0x22024000
            assert syms["__data_start"] == 0x2201b800, "diagnostic data layout changed"
            assert 0x22014000 <= syms["ble_trace_timer_store"] < 0x2201b800
        assert not any(s.startswith(("xTask", "vTask", "xQueue", "vQueue", "pvPort")) for s in syms), "RTOS symbol in BLE image"
        assert not any(n.startswith((".blerom", ".retention", ".tcm_data")) for n in sections), "uncopied vendor orphan section"
    flash = ".bootheader" in sections
    tcm_start = 0x22014000 if flash else 0x22010000
    assert syms["_start"] == tcm_start
    if flash:
        from flash_image import prepare
        prepared, image, boot_header = prepare(data)
        assert len(boot_header) == 176
        assert entry == syms["_flash_start"] == 0x23000000
        # Bootstrap copies only startup/vectors and writable initialized data.
        for section_name, source_symbol, base_symbol in [
            (".init", "__tcm_load_xip", "__tcm_start"),
            (".vectors", "__tcm_load_xip", "__tcm_start"),
            (".data", "__ocram_load_xip", "__ocram_start"),
        ]:
            sh = sections[section_name]
            offset = syms[source_symbol] - 0x23000000 + sh[3] - syms[base_symbol]
            assert image[offset:offset+sh[5]] == data[sh[4]:sh[4]+sh[5]], (path, "copy mismatch", section_name)
        for section_name in (".text", ".rodata"):
            sh = sections[section_name]
            assert 0x23000000 <= sh[3] < sh[3]+sh[5] <= 0x230ff000
            assert not sh[2] & 1, (path, "writable XIP section", section_name)
            offset = sh[3] - 0x23000000
            assert image[offset:offset+sh[5]] == data[sh[4]:sh[4]+sh[5]]
        assert 0x23000000 <= syms["__bl706_main"] < 0x230ff000
        assert tcm_start <= syms["bl706_irq_entry"] < 0x22020000
    else:
        assert entry == syms["_start"]
    assert syms["bl706_vectors"] % 64 == 0
    assert syms["bl706_fatal_trap"] % 64 == 0
    assert syms["__bss_end"] <= syms["__stack_bottom"]
    assert syms["__stack_top"] - syms["__stack_bottom"] == 8192
    assert syms["__stack_top"] <= 0x2202c000
    for symbol in ("bl706_clock_to_rc32m", "bl706_clock_apply"):
        assert tcm_start <= syms[symbol] < tcm_start + 0x4000, (path, "clock transition outside ITCM", symbol)
    if path.name == "flash_probe":
        for symbol in ("bl706_flash_blocking_read", "bl706_flash_blocking_write", "bl706_flash_blocking_erase", "bl706_flash_async_surface"):
            assert 0x22020000 <= syms[symbol] < 0x2202c000, (path, "compile-only flash surface missing/outside RAM", symbol)
    # Every load segment has VMA=LMA and stays in explicitly reserved RAM.
    for i in range(phnum):
        kind, _, va, pa, filesz, memsz, _, _ = struct.unpack_from("<8I", data, phoff+i*phsize)
        if kind != 1 or not memsz:
            continue
        if flash and filesz:
            assert 0x23000000 <= pa < pa+filesz <= 0x23100000
            if va == 0x21000000:
                assert filesz == memsz == 176
                continue
            if 0x23000000 <= va < 0x230ff000:
                assert pa == va + 0x1000 and va + memsz <= 0x230ff000
                continue
        else:
            assert va == pa
        ranges = [(tcm_start,0x22020000),(0x22020000,0x2202c000)]
        assert any(lo <= va and va+memsz <= hi for lo, hi in ranges), (path, hex(va), memsz)
    sh = sections[".vectors"]
    vectors = struct.unpack_from("<80I", data, sh[4])
    assert sh[5] == 320
    irq_sources = (3,7,41,45,48,50,52,53,60,72)
    assert all(vectors[i] == syms["bl706_irq_entry"] for i in irq_sources)
    assert all(v == syms["bl706_fatal_trap"] for i,v in enumerate(vectors) if i not in irq_sources)
    atomic = []
    float_ops = 0
    saved, restored = set(), set()
    for sh in headers:
        if not sh[2] & 4:
            continue
        pos, end = sh[4], sh[4]+sh[5]
        while pos < end:
            half = struct.unpack_from("<H", data, pos)[0]
            pc = sh[3] + pos - sh[4]
            if half & 3 != 3:
                q, funct = half & 3, half >> 13
                if q in (0,2) and funct in (1,3,5,7):
                    float_ops += 1
                    assert funct not in (1,5), (path, "unexpected compressed D instruction", hex(pc))
                if q == 2 and funct == 7: saved.add((half >> 2) & 31)
                if q == 2 and funct == 3: restored.add((half >> 7) & 31)
                pos += 2
            else:
                assert half & 0x1f != 0x1f, "unexpected >32-bit encoding"
                word = struct.unpack_from("<I", data, pos)[0]
                opcode = word & 0x7f
                if opcode == 0x2f: atomic.append(pc)
                if opcode in (0x07,0x27,0x43,0x47,0x4b,0x4f,0x53): float_ops += 1
                if opcode in (0x07,0x27):
                    assert (word >> 12) & 7 == 2, (path, "non-f32 FP load/store", hex(pc))
                if opcode in (0x43,0x47,0x4b,0x4f,0x53):
                    assert (word >> 25) & 3 == 0, (path, "non-f32 FP operation", hex(pc))
                if (word >> 15) & 31 == 2 and (word >> 12) & 7 == 2:
                    if opcode == 0x27: saved.add((word >> 20) & 31)
                    if opcode == 0x07: restored.add((word >> 7) & 31)
                pos += 4
    expected = {syms[n] for n in ("bl706_probe_lr", "bl706_probe_sc", "bl706_probe_amo_insn")} if probes else set()
    assert set(atomic) == expected and len(atomic) == len(expected), (path, "unexpected A opcodes", atomic)
    if fpu:
        assert float_ops > 0
        assert saved == restored == set(range(32)), (path, "FP context save/restore incomplete", saved, restored)
    else:
        assert float_ops == 0, (path, "unexpected FP opcode", float_ops)
    print(f"{path}: {'XIP+RAM' if flash else 'RAM'}/vector/ABI checks OK; A={len(atomic)}, FP={float_ops}, stack=8192")

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path", nargs="?", type=Path, default=ROOT/"target/riscv32imfc-unknown-none-elf/release/pinecil-rs")
    parser.add_argument("--ble-placement", choices=("itcm", "xip"), default="itcm")
    args = parser.parse_args()
    audit(args.path.resolve(), fpu="riscv32imfc-unknown-none-elf" in str(args.path), ble_placement=args.ble_placement)

"""Read RTT and bounded PD I2C diagnostics through a running OpenOCD TCL server."""
import argparse
import socket
import struct
import subprocess
import time
import json
import sys

p = argparse.ArgumentParser()
p.add_argument('--start-pd', action='store_true')
p.add_argument('--seconds', type=float, default=15)
p.add_argument('--command', type=int, choices=(1, 2), default=1, help='1: start PD; 2: test I2C cancellation, then wait again')
p.add_argument('--trace-output')
p.add_argument('elf', nargs='?', default='target/riscv32imfc-unknown-none-elf/release/pinecil-rs')
a = p.parse_args()
symbols = {}
for line in subprocess.check_output(['readelf', '-sW', a.elf], text=True).splitlines():
    fields = line.split()
    if len(fields) >= 8 and fields[-1] in ('PD_START', '_SEGGER_RTT', 'PD_TRACE', 'PD_TRACE_COUNT'):
        symbols[fields[-1]] = int(fields[1], 16)
control = symbols['_SEGGER_RTT']
start_flag = symbols.get('PD_START')
s = socket.create_connection(('127.0.0.1', 6666), 3)
s.settimeout(2)

def cmd(text):
    s.sendall(text.encode() + b'\x1a')
    data = b''
    while not data.endswith(b'\x1a'):
        part = s.recv(65536)
        if not part:
            raise EOFError('OpenOCD closed connection')
        data += part
    result = data[:-1].decode()
    return result

def words(addr, n):
    result = cmd(f'read_memory {addr:#x} 32 {n}')
    try:
        values = [int(x, 0) for x in result.split()]
        assert len(values) == n
        return values
    except (ValueError, AssertionError):
        raise RuntimeError(result)

def write(addr, value):
    result = cmd(f'write_memory {addr:#x} 32 {{{value:#x}}}')
    if result.strip():
        raise RuntimeError(result)

def read_bytes(addr, n):
    aligned = addr & ~3
    skip = addr - aligned
    data = b''.join(struct.pack('<I', w) for w in words(aligned, (skip+n+3)//4))
    return data[skip:skip+n]

def drain():
    name, buf, size, wr, rd, flags = words(control+24, 6)
    assert size in (4096, 16384) and wr < size and rd < size, (name, buf, size, wr, rd, flags)
    if wr == rd:
        return
    if wr > rd:
        data = read_bytes(buf + rd, wr-rd)
    else:
        data = read_bytes(buf+rd, size-rd) + read_bytes(buf, wr)
    print(data.decode(errors='replace'), end='', flush=True)
    write(control+40, wr)

events = []
trace_count = 0
def trace():
    global trace_count
    if not a.trace_output or 'PD_TRACE' not in symbols:
        return
    trace_count = max(trace_count, words(symbols['PD_TRACE_COUNT'], 1)[0])
    n = min(trace_count, 256) - len(events)
    if n <= 0:
        return
    block = words(symbols['PD_TRACE'] + len(events)*44, n*11)
    for j in range(n):
        start, end, meta, *data = block[j*11:(j+1)*11]
        if end == 0:
            break
        events.append(dict(index=len(events), start_us=start, end_us=end,
            duration_us=(end-start)&0xffffffff, address=meta&127,
            register=(meta>>8)&255, read=bool(meta&(1<<16)),
            length=(meta>>17)&8191, error=bool(meta&(1<<30)),
            cancelled=bool(meta&(1<<31)),
            hex=b''.join(struct.pack('<I',x) for x in data).hex()))

capture_error = None
try:
    print(f'Capture host monotonic={time.monotonic():.6f}', flush=True)
    drain()
    if a.start_pd:
        assert start_flag is not None, 'ELF has no PD_START; build with pd-manual-start'
        print(f'Writing PD_START={a.command} at host monotonic={time.monotonic():.6f}', flush=True)
        write(start_flag, a.command)
    deadline = time.monotonic() + a.seconds
    while time.monotonic() < deadline:
        trace()
        drain()
        time.sleep(.02)
except Exception as e:
    capture_error = e
    print(f'\nCAPTURE ERROR at host monotonic={time.monotonic():.6f}: {e!r}', flush=True)
finally:
    if a.trace_output and 'PD_TRACE' in symbols:
        try:
            trace()
        except Exception as e:
            print(f'TRACE ERROR: {e!r}', flush=True)
        try:
            with open(a.trace_output, 'w') as f:
                json.dump(dict(count=trace_count, events=events), f, indent=2)
            print(f'Trace saved: {len(events)} of {trace_count} operations', flush=True)
        except Exception as e:
            print(f'TRACE ERROR: {e!r}', flush=True)
    s.close()

if capture_error is not None:
    sys.exit(1)

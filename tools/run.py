"""Cargo runner for BL706 RAM/flash ELFs and probe-rs RTT."""
from pathlib import Path
import os
import sys
from flash_image import write_prepared

elf = write_prepared(Path(sys.argv[1]))
args = ["probe-rs", "run", "--chip", "BL706", "--protocol", "jtag",
        "--speed", "1000", "--verify", str(elf), *sys.argv[2:]]
os.execvp(args[0], args)

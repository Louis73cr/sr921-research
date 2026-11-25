#!/usr/bin/env python3
"""Interrupt U-Boot autoboot (bootdelay=0) and run read-only recon commands.

Flood `space`+Ctrl-C at power-on, catch the `=>` prompt, then dump environment,
partition map and the HAB secure-boot fuse. Nothing is written.

Usage: uboot_console.py [DEV] [LOGFILE]
Power-cycle the board after launching; this waits up to 5 min for the prompt.
"""
import sys, time
from serial import open_port, read_until, send

DEV = sys.argv[1] if len(sys.argv) > 1 else "/dev/ttyUSB0"
LOG = open(sys.argv[2], "wb") if len(sys.argv) > 2 else None

# All read-only. `fuse read 0 6 1` = OCOTP_CFG5; bit1 (SEC_CONFIG) 0 => HAB open.
CMDS = ["version", "printenv", "mmc part", "fuse read 0 6 1"]

fd = open_port(DEV)
buf, deadline = b"", time.time() + 300
while b"=>" not in buf and time.time() < deadline:
    buf += send(fd, " \x03", timeout=0.01, sink=LOG)   # space + Ctrl-C
if b"=>" not in buf:
    sys.exit("no U-Boot prompt (power-cycle the board while this runs)")

print("[uboot] prompt caught")
for c in CMDS:
    out = send(fd, c, token="=>", timeout=4, sink=LOG)
    sys.stdout.write(out.decode(errors="replace").replace("\r", ""))

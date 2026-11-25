#!/usr/bin/env python3
"""Minimal raw-termios serial driver for the SR921/VR921 debug UART (X10).

115200 8N1, 3.3 V. Wiring: adapter GND->GND pad, adapter RX->board TX pad
(~3.19 V idle), adapter TX->board RX pad (~3.27 V idle). Use a 3.3 V adapter.

Importable helpers used by the other tools; run directly for a dumb terminal.
"""
import os, sys, termios, time, select


def open_port(dev="/dev/ttyUSB0", baud=termios.B115200):
    fd = os.open(dev, os.O_RDWR | os.O_NOCTTY)
    a = termios.tcgetattr(fd)
    a[0] = a[1] = a[3] = 0                      # raw iflag/oflag/lflag
    a[2] = termios.CS8 | termios.CREAD | termios.CLOCAL
    a[4] = a[5] = baud
    termios.tcsetattr(fd, termios.TCSANOW, a)
    return fd


def read_until(fd, token=None, timeout=5.0, sink=None):
    """Read until `token` (bytes) seen in the tail, or timeout. Returns buffer."""
    buf, end = b"", time.time() + timeout
    while time.time() < end:
        if select.select([fd], [], [], 0.05)[0]:
            d = os.read(fd, 4096)
            buf += d
            if sink:
                sink.write(d); sink.flush()
            if token and token in buf[-len(token) - 256:]:
                break
    return buf


def send(fd, line, token=None, timeout=5.0, sink=None):
    os.write(fd, line.encode() + b"\r")
    return read_until(fd, token, timeout, sink)


if __name__ == "__main__":
    dev = sys.argv[1] if len(sys.argv) > 1 else "/dev/ttyUSB0"
    fd = open_port(dev)
    print(f"[serial] {dev} 115200 8N1 — Ctrl-C to quit")
    try:
        while True:
            buf = read_until(fd, timeout=0.2)
            if buf:
                sys.stdout.write(buf.decode(errors="replace"))
                sys.stdout.flush()
    except KeyboardInterrupt:
        pass

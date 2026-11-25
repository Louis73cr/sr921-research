#!/usr/bin/env python3
"""Minimal YMODEM sender over an already-open raw serial fd (for U-Boot `loady`).

No external tools (lrzsz not needed). 1 KiB blocks, CRC16/XMODEM. Enough to feed
U-Boot's ymodem receiver; not a general-purpose implementation.
"""
import os, time, select


def _crc16(data: bytes) -> int:
    crc = 0
    for b in data:
        crc ^= b << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


def _getc(fd, timeout):
    if select.select([fd], [], [], timeout)[0]:
        return os.read(fd, 1)
    return b""


def _wait(fd, want: bytes, timeout: float) -> bool:
    end = time.time() + timeout
    while time.time() < end:
        c = _getc(fd, end - time.time())
        if c == want:
            return True
    return False


def _block(seq: int, payload: bytes) -> bytes:
    assert len(payload) == 1024
    hdr = bytes([0x02, seq & 0xFF, (~seq) & 0xFF])          # STX
    crc = _crc16(payload)
    return hdr + payload + bytes([crc >> 8, crc & 0xFF])


def send(fd, path, log=None):
    data = open(path, "rb").read()
    name = os.path.basename(path).encode()
    # block 0: filename + size
    info = name + b"\x00" + str(len(data)).encode() + b"\x00"
    info = (info + b"\x00" * 1024)[:1024]
    if not _wait(fd, b"C", 20):
        raise TimeoutError("no 'C' from receiver (is `loady` running?)")
    for _ in range(10):
        os.write(fd, _block(0, info))
        if _getc(fd, 3) == b"\x06":
            break
    else:
        raise IOError("block0 not ACKed")
    _wait(fd, b"C", 5)
    # data blocks
    seq = 1
    for off in range(0, len(data), 1024):
        chunk = data[off:off + 1024]
        chunk = chunk + b"\x1a" * (1024 - len(chunk))
        for attempt in range(8):
            os.write(fd, _block(seq, chunk))
            if _getc(fd, 3) == b"\x06":
                break
        else:
            raise IOError(f"block {seq} not ACKed")
        seq += 1
        if log and seq % 64 == 0:
            log.write(f"  ymodem {off//1024+1}/{(len(data)+1023)//1024} blocks\n".encode()); log.flush()
    # EOT
    for _ in range(3):
        os.write(fd, b"\x04")
        if _getc(fd, 3) in (b"\x06", b""):
            break
    # end-of-batch: empty filename block
    _wait(fd, b"C", 5)
    os.write(fd, _block(0, b"\x00" * 1024))
    _getc(fd, 3)
    return len(data)

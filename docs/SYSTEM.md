# System internals

Captured from a one-shot root shell. No device-unique values are reproduced here.

## Distro & kernel

- **Yocto / Poky 5.0.18 "scarthgap"**, `ID=vaillant`,
  `CPE_NAME=cpe:/o:openembedded:vaillant:5.0.18`.
- Kernel **Linux 6.1.176-sde-comdialog2**, built 2026-06, GCC 13.4 (poky).
- Machine model: `Vaillant HEM 2.0 i.MX6` (i.MX6DL silicon rev 1.3).
- Root is **squashfs, read-only**. `/var` and `rootfs_b` are **f2fs** (U-Boot can't read
  f2fs, which is why `ls mmc 0:5/0:8` reports "Unrecognized filesystem" — not encryption).

## eMMC layout (`/dev/mmcblk0`, 3.56 GiB) + fstab

| Part | Mount | FS | Notes |
|------|-------|----|-------|
| p1 | `/appconfig` | ext2 ro | **device identity** (cloud cert/key, EEBUS `spine/`, gpg, serial.conf). ~19 MiB, 1% used |
| p2 / p3 | `/media/kernel_1` / `_2` | ext2 | FIT kernels A/B |
| p5 | `/media/rootfs_b` | f2fs | rootfs slot (writable) |
| p6 / p7 | `/media/rootfs_1` / `_2` | squashfs ro | **rootfs A/B** (active = p7) |
| p8 | `/var` | f2fs | live data, logs |

Plus `mmcblk0boot0/1` (2 MiB each eMMC boot partitions, identical content).

## Access facts

- root password is **locked** (`root:*` in shadow). No `sshd`/`dropbear`/`ssh` installed.
- `serial-getty@ttymxc0` **is** enabled — a login console would appear if the UART were
  enabled in the DTB, but login still needs a password we don't have. Hence the
  `init=/bin/sh` route.
- Present tools: `busybox`, `nc` (OpenBSD-less; BusyBox nc), `dd`, `gzip`, `tar`, `ip`,
  `udhcpc`, `f2fscrypt`/`e4crypt`. **Absent**: `curl`, `python3`, `socat`, `ssh`.
- `udhcpc` with no `-s` script does **not** apply the lease; set the IP by hand
  (`ip addr add …/24 dev eth0`) or provide a bound-hook script on a writable tmpfs.
- Watchdog at `/dev/watchdog` (hardware; `watchdog.conf` + `vg-watchdog.service`).

## Vaillant services (systemd, `vg-*`)

The stack is a set of D-Bus agents. Most relevant:

| Unit | D-Bus name | Role |
|------|-----------|------|
| `vg-ebus_router.service` | `de.vaillant.ebus_router` | **eBUS router** — central hub; `/usr/bin/vaillantgroup/ebus_router_agent`, RT fifo prio 90. "supports phy operator 2020 and 2010" |
| `vg-binding_vrc700.service` | `de.vaillant.binding_vrc700` | abstracts the **VRC 700** system controller to the backend |
| `vg-hemagent.service` | `de.vaillant.hem_agent` | energy management (EEBUS/HEM) |
| `vg-spineagent` / `vg-spineadapter` | — | EEBUS/SPINE stack |
| `vg-asdagent`, `vg-databaseagent`, `vg-ec`, `vg-ep`, `vg-gca`, `vg-rbr`, `vg-scenariomanager`, `vg-systemtimeagent`, `vg-systemmonitoragent` | `de.vaillant.*` | supporting agents |
| `mender-client.service` | — | **OTA updater** (Vaillant-controlled; target of the block-OTA option) |
| `avahi-daemon`, `dnsmasq`, `ntpd`, `fluent-bit` | — | net / mDNS / logging-to-cloud |

Binaries live under `/usr/bin/vaillantgroup/` (not `/opt`). Boot target is
`vg-vr920_wo_cc1310.target`.

## Why this matters

`de.vaillant.ebus_router` is the single choke point every agent uses to reach the eBUS.
A new D-Bus client that subscribes there can observe (and, if the API allows, send) eBUS
traffic **alongside** the stock software — the basis of the HA bridge without replacing
anything. The exact router API (raw eBUS vs. high-level only) is the next thing to probe,
which needs persistent access (SSH) while the stack is running.

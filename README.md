# SR921 / VR921 — gateway research

Reverse-engineering notes and tooling for the **Saunier Duval MiLink v3 (SR921)**
heat-pump internet gateway, which is the twin of the **Vaillant VR921 / sensoNET**
(same Diehl Controls board, same firmware, board self-identifies as
`Vaillant VR921` / `Vaillant HEM 2.0 i.MX6`).

Goal: talk to the heat pump **locally** (read sensors, write setpoints/modes via the
eBUS) and expose it to **Home Assistant**, **without losing the Vaillant cloud** — the
stock software keeps running and stays connected. This is personal interoperability
work on owned hardware.

> **No proprietary firmware here.** The Vaillant root filesystem, kernel and binaries
> are not redistributed. This repo contains only original tooling, documentation, and
> our own add-on. All device-unique secrets (cloud cert/key, MAC, serial, EEBUS SKI)
> have been scrubbed.

## Status

| Step | State |
|------|-------|
| Board teardown & chip ID | ✅ done — [docs/HARDWARE.md](docs/HARDWARE.md) |
| Serial console located (UART1, disabled in DTB) | ✅ done |
| U-Boot access, secure boot (HAB) check | ✅ **HAB open** — [docs/BOOT.md](docs/BOOT.md) |
| Root shell via one-shot `init=/bin/sh` + in-RAM DTB patch | ✅ done |
| System map (Yocto, partitions, services, eBUS router) | ✅ done — [docs/SYSTEM.md](docs/SYSTEM.md) |
| Full eMMC backup (device-local, private) | ✅ done |
| Persistent add-on: SSH + status entities + block OTA | ⏳ in progress — [docs/ROADMAP.md](docs/ROADMAP.md) |
| eBUS → Home Assistant bridge | ⏳ planned |

## Hardware at a glance

- SoC: NXP **i.MX6 Solo** (`MCIMX6S5EVM10AC`), 792 MHz, 512 MiB DDR3L
- eMMC: 3.56 GiB, A/B layout (Mender OTA)
- Wi-Fi: SN8000 · Ethernet: 100 Mbit (Micrel KSZ8081 PHY + Pulse HU1288ENL)
- eBUS: 2-pin green terminal, opto-isolated (2× SFH6156) → handled by a separate MCU
- Debug: **X10**, 3 gold pads on the board edge = UART1 console, 3.3 V, 115200 8N1
- PCB: Diehl Controls, ref `0020261008`

## Target architecture

```
PAC / controller ──eBUS──► SR921 gateway
                             ├─ Vaillant stack (untouched) ──► Vaillant cloud + app
                             └─ our bridge ──TCP──► Home Assistant (ebusd add-on) ──MQTT──► entities
```

See [docs/ROADMAP.md](docs/ROADMAP.md) for the add-on design (overlay that survives
OTA, cloud guaranteed).

## Layout

- `docs/` — findings: hardware, boot chain, system internals, roadmap
- `tools/` — original scripts: serial driver, U-Boot console helper, one-shot root shell

## License

Original code and docs: MIT (see [LICENSE](LICENSE)). Vaillant/Saunier Duval are
trademarks of their owners; this project is not affiliated with or endorsed by them.

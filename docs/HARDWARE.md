# Hardware

Board from a Saunier Duval MiLink v3 case. Label: `SR 921`, ref `0020261008`,
`5…24 V ⎓ 3 W`, IP21, made by SDECCI SAS (Diehl group), Nantes. PCB silk:
`DIEHL Controls`, `OC-138 94V-0`.

## Main components

| Ref | Part | Role |
|-----|------|------|
| U (BGA, top) | NXP **MCIMX6S5EVM10AC** (i.MX6 Solo, ARMv7, 792 MHz) | main SoC, runs Linux |
| 2× | **Nanya NT5CC128M16** DDR3L | RAM, 512 MiB total (one per side) |
| Micron (BGA) | marking `…JY995` | eMMC, 3.56 GiB |
| top-left shield | **SN8000** (FCC `QPUSN8000`) | Wi-Fi module |
| X2 + Pulse **HU1288ENL** | Ethernet 100 Mbit (Micrel KSZ8081 PHY) | LAN |
| X3 | barrel jack | power in, 5–24 V |
| green 2-pin terminal (top) | eBUS | 2-wire bus to heat pump |
| 2× **SFH6156** optocouplers + diode bridge | eBUS isolation | polarity-independent |
| QFP ~80-pin (bottom side) | eBUS MCU | handles eBUS PHY, talks to SoC over a UART |
| bottom, empty shield frame | unpopulated **CC1310** radio | `w/o RADIO1` variant |

## Debug / test points

- **X10** — 3 gold edge pads, board left edge. Not a serial console by default
  (disabled in the DTB), but the pins are UART1 (`serial@2020000`):
  - GND
  - **TX** ≈ 3.19 V idle (SoC → us)
  - **RX** ≈ 3.27 V idle (us → SoC)
  - 115200 8N1, 3.3 V logic.
- A small tactile button near the Ethernet jack: SoC reset / boot-mode (untested).
- The large button is the user button (WPS / factory reset) — long-press wipes Wi-Fi,
  cloud pairing and EEBUS trust; avoid.

## eBUS wiring plan (fallback, cloud-independent)

A commercial eBUS adapter (ebusd adapter v5, Elecrow C6, or an ESP32+opto DIY) wires
in **parallel** on the green 2-pin terminal. The gateway can stay in place; multiple
devices share the bus. This is the robust path and does not depend on any firmware mod.

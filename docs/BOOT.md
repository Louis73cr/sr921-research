# Boot chain & access

## Serial console

The debug UART (X10) is **disabled in the device tree** that Vaillant ships
(`status = "disabled"` on `serial@2020000`, i.e. serial0 alias). U-Boot still uses it,
so you see SPL + U-Boot + early kernel, then it goes silent once Linux applies the DTB.

- `tools/serial.py` — minimal raw-termios serial driver (115200 8N1).
- `tools/uboot_console.py` — spam `space`+`Ctrl-C` at power-on to stop autoboot
  (`bootdelay=0`, so you must flood keys as power comes up), then run read-only commands.

## U-Boot

`U-Boot 2019.07`, board `Vaillant VR921 (w/o RADIO1)`, i.MX6 Solo, 512 MiB, boot from
eMMC. Key environment:

- A/B via Mender: `mender_boot_part` 6/7, kernels on `mmc 0:2` / `mmc 0:3`.
- `bootcmd` runs `check_firstboot` → **`saveenv`** on every boot (bumps bootcount), so
  to boot a one-off config without persisting, use `run do_boot`, not `boot`.
- `fuse read 0 6 1` → `0x00000010`. Bit `SEC_CONFIG[1]` = 0 ⇒ **HAB (secure boot) is
  OPEN**: the SoC will boot any (even unsigned) bootloader. No signature barrier.
- FIT images verify a **sha256 hash only** (`sha256+`), **no RSA signature** — U-Boot
  does not check *who* built the kernel.

## Root shell (no writes)

The kernel is reused as-is; only the DTB is patched **in RAM** to re-enable the console
UART, then booted with `init=/bin/sh`. Nothing is written to eMMC (env not saved, DTB
edited in memory). See `tools/root_shell.py`.

Sequence at the `=>` prompt:

```
load mmc 0:3 ${loadaddr} fitImage
imxtract ${loadaddr} fdt-<board>.dtb ${fdt_addr}
fdt addr ${fdt_addr}; fdt resize 1024
fdt set /soc/bus@2000000/spba-bus@2000000/serial@2020000 status okay
setenv bootargs console=ttymxc0,115200 earlycon=ec_imx6q,0x02020000,115200 \
       keep_bootcon loglevel=8 root=/dev/mmcblk0p7 ro rootfstype=squashfs rootwait init=/bin/sh
bootm ${loadaddr}:kernel-1 - ${fdt_addr}
```

Feed the watchdog from the shell or it resets after ~60 s:
`exec 3>/dev/watchdog; (while :; do printf . >&3; sleep 5; done) &`

The kernel has initrd/initramfs support (`unpack_to_rootfs`, `gunzip`/`unlz4`/`unxz`
present) and `BLK_DEV_INITRD` symbols — usable for an overlay init. No `/proc/config.gz`.

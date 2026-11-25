# Roadmap — persistent add-on (cloud-safe)

Design goal: add our features **without modifying the Vaillant rootfs**, so a Vaillant
OTA can never leave the gateway worse than stock. If our layer ever fails, the box must
still boot the Vaillant stack and keep the cloud. Constraint set by the owner:
**never lose the Vaillant cloud; default state must be restorable.**

## Proven: custom kernel+initramfs boots, SSH works (RAM-only)

Milestone reached with **zero eMMC writes**: U-Boot loads the stock FIT kernel, a
RAM-patched DTB (re-enabling the debug UART), and our cross-built initramfs over TFTP,
then boots it. A static ARMv7 dropbear starts and accepts key auth. Confirms HAB is open,
FIT needs no signature, and our toolchain targets the right ABI. See
`firmware/build_initramfs.sh` + `tools/ram_ssh_test*`.

Lessons: busybox/dropbear must be **ARMv7** (not the amd64 build host); this busybox has
`ifconfig` but no `ip`; `/init` parses the IP from the kernel cmdline in pure shell.

## Persistence approach: init-wrapper + bind-mounts (NOT overlayfs)

Recon finding: **the Vaillant kernel has no overlayfs** (not built-in, no `overlay.ko`).
So we persist via `mount --bind` (confirmed working) instead:

- Keep our files (dropbear, host/authorized keys, our `.service` units, status reporter)
  on a writable partition the stock rootfs doesn't use (p5/p8, f2fs).
- A small `init=` wrapper, injected via the **U-Boot `optargs` env** (one reversible env
  write; full `printenv` is backed up), runs before systemd: it bind-mounts our unit
  files into the systemd `.wants` dirs on the read-only squashfs, bind-mounts `/dev/null`
  over `mender-client.service` to **block OTA on this boot only**, then execs the real
  `/lib/systemd/systemd`.
- The Vaillant stack starts normally; cloud + app unaffected. Stock slot (p7) is never
  edited → instant rollback by clearing the env var. If a future OTA changes U-Boot and
  drops `optargs`, the box simply boots stock (cloud kept, our layer gone until re-added).

### Trade-offs vs. Option A (block OTA outright)

- **A** = disable `mender-client`, install our rootfs. Simple, but no more Vaillant
  updates → cloud may eventually reject an old version. Rejected as default.
- **B** = survives OTA, keeps updates. More work; needs a one-time write to the U-Boot
  env and to p1. Backup covers everything. **Chosen.**

> Residual risk, documented honestly: Vaillant *controls and can force* OTAs. A future
> OTA could ship a new U-Boot that drops our env hook, or even blow the HAB fuses to
> close secure boot. Cloud would keep working; our access would not. The only truly
> vendor-independent control path remains a **parallel eBUS adapter** on the green
> terminal — keep that as the reliable option.

## Requested features

1. **SSH (dropbear)** with the owner's public key — maintenance without the serial jig.
2. **Status entities in Home Assistant**, published over MQTT discovery:
   - gateway **IP address**
   - gateway **uptime** / "up since"
   - reachability (online/offline)
   These come from a small reporter on the gateway (or, until the overlay lands, a
   HA-side `ping`/MQTT helper).
3. **Block Vaillant OTA** on the modified slot only, while keeping the stock slot intact:
   mask `mender-client.service` in the overlay (not deleted from the stock rootfs), so
   removing the overlay restores stock OTA behaviour.
4. **eBUS → HA bridge** (next phase): a D-Bus client on `de.vaillant.ebus_router` that
   re-exposes the bus to **ebusd** (enhanced-protocol TCP, like a commercial adapter) →
   ebusd decodes Vaillant/Saunier Duval → **MQTT discovery** → HA entities (flow temp,
   outdoor temp, DHW, power, COP if available; setpoints/modes writable).

## Build / test discipline

- Everything is first **loaded over the network into RAM and tested** (as the recon was),
  writing to eMMC only once a step is proven.
- First write target is the **inactive rootfs slot (p6)** + U-Boot env to boot it; the
  active Vaillant slot (p7) stays untouched for instant rollback.
- Keep the private full-image backup current before any write.

## Still needed from hardware

- Exact **heat-pump model** and **controller (VRC 700?) version** — required for ebusd
  decode files. (`binding_vrc700` strongly suggests a VRC 700-class controller.)

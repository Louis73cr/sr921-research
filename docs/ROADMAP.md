# Roadmap — persistent add-on (cloud-safe)

Design goal: add our features **without modifying the Vaillant rootfs**, so a Vaillant
OTA can never leave the gateway worse than stock. If our layer ever fails, the box must
still boot the Vaillant stack and keep the cloud. Constraint set by the owner:
**never lose the Vaillant cloud; default state must be restorable.**

## Chosen approach: Option B — overlay init

Insert a tiny stage between U-Boot and the Vaillant userland that overlays our additions
on top of the read-only squashfs (`overlayfs`), from files kept on a partition OTA does
not touch. Nothing in the stock rootfs is edited.

- Our files (dropbear, keys, status reporter, eBUS bridge) live outside the A/B rootfs.
- `overlayfs` (kernel supports it) merges them read-only over `/`.
- The Vaillant stack starts normally; cloud + app unaffected.
- On OTA: Vaillant replaces the rootfs; our overlay re-applies on next boot. If a future
  OTA breaks the hook, it is designed to **fall through to a normal boot** (cloud kept,
  our layer lost until re-applied).

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

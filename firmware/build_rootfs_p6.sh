#!/bin/sh
# Build a modified rootfs for the INACTIVE A/B slot (p6): stock Vaillant squashfs
# + our SSH add-on + OTA block, nothing in the active slot touched. Run as root
# (Docker) so ownership/permaddons are preserved.
#   /in/p7.squashfs  stock rootfs (extracted from the device backup)
#   /files           dropbear/dropbearkey (ARM static) + authorized_keys
#   /out             output dir -> rootfs_p6.squashfs
set -eu
IN=/in/p7.squashfs; OUT=/out/rootfs_p6.squashfs; F=/files
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq; apt-get install -y -qq squashfs-tools >/dev/null

R=/work/rootfs; rm -rf /work; mkdir -p /work
unsquashfs -d "$R" "$IN" >/dev/null

# --- our add-on, all additive (no stock file overwritten) ---
install -m755 "$F/sbin/dropbear"    "$R/usr/sbin/dropbear"
install -m755 "$F/bin/dropbearkey"  "$R/usr/bin/dropbearkey"
install -d -m700 "$R/root/.ssh"
install -m600 "$F/authorized_keys"  "$R/root/.ssh/authorized_keys"

# host key on persistent /var (p8); /var/lib may be volatile tmpfs, so use /var/sr921
cat > "$R/etc/systemd/system/sr921-sshd.service" <<'EOF'
[Unit]
Description=SR921 add-on SSH (dropbear, key-only)
# No After= on vg-*/network targets: those Require multi-user.target and would form
# an ordering cycle (systemd then drops us). dropbear binds fine before the net is up.
After=basic.target
[Service]
ExecStartPre=/bin/mkdir -p /var/sr921
ExecStartPre=/bin/sh -c '[ -f /var/sr921/hostkey ] || /usr/bin/dropbearkey -t ecdsa -f /var/sr921/hostkey'
ExecStart=/usr/sbin/dropbear -F -E -r /var/sr921/hostkey -p 22
Restart=always
RestartSec=5
[Install]
WantedBy=multi-user.target
EOF
ln -sf ../sr921-sshd.service "$R/etc/systemd/system/multi-user.target.wants/sr921-sshd.service"

# Static dropbear can't dlopen NSS modules (compat/systemd), so getpwnam(root) fails
# and SSH closes before auth. Put `files` first (root lives in /etc/passwd); dynamic
# Vaillant binaries are unaffected (same users, modules remain as fallback).
cat > "$R/etc/nsswitch.conf" <<'EOF'
passwd:         files compat systemd
group:          files compat systemd
shadow:         files compat systemd
hosts:          files myhostname mdns4_minimal [NOTFOUND=return] mdns6_minimal [NOTFOUND=return] dns
networks:       files
protocols:      db files
services:       db files
ethers:         db files
rpc:            db files
netgroup:       nis
EOF

# block Vaillant OTA on THIS slot only, WITHOUT breaking the boot target that
# Requires mender-client: keep the unit present+active but replace its command with
# an inert loop (mender never runs). Masking (/dev/null) would fail target isolation.
install -d "$R/etc/systemd/system/mender-client.service.d"
cat > "$R/etc/systemd/system/mender-client.service.d/override.conf" <<'EOF'
# sr921 add-on: neutralize OTA. The unit stays satisfiable for vg-vr920*.target,
# but the real `mender daemon` is never executed. Remove this file to restore OTA.
[Service]
ExecStartPre=
ExecStart=
ExecStartPost=
ExecStart=/bin/sh -c 'while :; do sleep 3600; done'
Restart=always
EOF

mksquashfs "$R" "$OUT" -comp gzip -b 131072 -noappend -no-progress >/dev/null
echo "[rootfs] $(ls -l "$OUT" | awk '{print $5}') bytes -> $OUT"

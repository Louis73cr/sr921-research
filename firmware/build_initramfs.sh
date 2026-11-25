#!/bin/sh
# Cross-build a static armv7 dropbear and assemble a minimal initramfs that:
#   - brings up eth0 (static IP from $BOARD_IP/$MASK)
#   - starts dropbear with an authorized key
#   - feeds the hardware watchdog
# This initramfs is for RAM-only testing (U-Boot tftpboot) FIRST. No eMMC write.
#
# Run inside debian:bookworm (the Makefile target wraps this in Docker):
#   firmware/build_initramfs.sh /out /pubkey
set -eu
OUT="${1:?out dir}"; PUBKEY_SRC="${2:?authorized_keys path}"
DROPBEAR_VER=2022.83
HOST=arm-linux-gnueabihf
JOBS=$(nproc)

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq gcc-${HOST} make wget zlib1g-dev cpio xz-utils busybox-static >/dev/null

work=/tmp/w; mkdir -p "$work"; cd "$work"
wget -q "https://matt.ucc.asn.au/dropbear/releases/dropbear-${DROPBEAR_VER}.tar.bz2"
tar xf "dropbear-${DROPBEAR_VER}.tar.bz2"; cd "dropbear-${DROPBEAR_VER}"

# static, no zlib (simpler), only needed programs
./configure --host=${HOST} --disable-zlib --enable-static \
    CC=${HOST}-gcc >/dev/null
make -j"$JOBS" PROGRAMS="dropbear dropbearkey scp" STATIC=1 >/dev/null
${HOST}-strip dropbear dropbearkey scp

# ---- assemble initramfs root ----
R="$work/root"; rm -rf "$R"; mkdir -p "$R"/{bin,sbin,etc/dropbear,proc,sys,dev,root/.ssh,var/run}
cp dropbear "$R/sbin/"; cp dropbearkey scp "$R/bin/"
cp /usr/bin/busybox "$R/bin/busybox"
for a in sh mount ip ifconfig sleep cat ls mkdir ln; do ln -sf busybox "$R/bin/$a"; done
cp "$PUBKEY_SRC" "$R/root/.ssh/authorized_keys"; chmod 600 "$R/root/.ssh/authorized_keys"

cat > "$R/init" <<'INIT'
#!/bin/sh
BOARD_IP="__BOARD_IP__"; MASK="__MASK__"
mount -t proc proc /proc; mount -t sysfs sys /sys; mount -t devtmpfs dev /dev 2>/dev/null
# keep the hardware watchdog happy
( exec 3>/dev/watchdog; while :; do printf . >&3; sleep 5; done ) &
ip link set eth0 up; ip addr add "$BOARD_IP/$MASK" dev eth0
[ -f /etc/dropbear/dropbear_ecdsa_host_key ] || /bin/dropbearkey -t ecdsa -f /etc/dropbear/dropbear_ecdsa_host_key
echo "[initramfs] dropbear on $BOARD_IP :22 — RAM only, nothing written to eMMC"
/sbin/dropbear -r /etc/dropbear/dropbear_ecdsa_host_key -E -F
INIT
chmod +x "$R/init"

( cd "$R" && find . | cpio -o -H newc --quiet | gzip -9 ) > "$OUT/initramfs.cpio.gz"
echo "[build] $(du -h "$OUT/initramfs.cpio.gz" | cut -f1) initramfs.cpio.gz"

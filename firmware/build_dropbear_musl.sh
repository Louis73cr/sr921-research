#!/bin/sh
# Cross-build a static ARM hardfloat dropbear against MUSL (avoids glibc-static
# edge cases; matches the known-good busybox musl build). Outputs to /out.
set -eu
OUT="${1:?out dir}"
DROPBEAR_VER=2022.83
TC=arm-linux-musleabihf            # hardfloat, matches device (e_flags 0x400)
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq wget bzip2 make xz-utils ca-certificates >/dev/null

cd /tmp
wget -q "https://musl.cc/${TC}-cross.tgz"
tar xf "${TC}-cross.tgz"
export PATH="/tmp/${TC}-cross/bin:$PATH"

wget -q "https://matt.ucc.asn.au/dropbear/releases/dropbear-${DROPBEAR_VER}.tar.bz2"
tar xf "dropbear-${DROPBEAR_VER}.tar.bz2"
cd "dropbear-${DROPBEAR_VER}"
./configure --host=${TC} --disable-zlib CC=${TC}-gcc >/dev/null
cat > localoptions.h <<'LOCAL'
#define DROPBEAR_SVR_PASSWORD_AUTH 0
#define DROPBEAR_SVR_PUBKEY_AUTH 1
#define DROPBEAR_CLIENT 0
LOCAL
make -j"$(nproc)" PROGRAMS="dropbear dropbearkey" STATIC=1 >/dev/null
${TC}-strip dropbear dropbearkey
cp dropbear dropbearkey "$OUT/"
echo "[dropbear-musl] $(ls -l "$OUT/dropbear" | awk '{print $5}') bytes"

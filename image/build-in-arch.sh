#!/bin/bash
# Builds the LinuxAI ISO. Must run on Arch Linux as root (build.sh runs it in a container for you).
#   SRC=<repo checkout>  OUT=<output dir>  WORK=<scratch dir>
set -euo pipefail
SRC=${SRC:-/src}; OUT=${OUT:-/out}; WORK=${WORK:-/work}
VERSION=${LINUXAI_VERSION:-0.1.0}

if [ -n "${EXTRA_CA:-}" ] && [ -f "$EXTRA_CA" ]; then   # only for networks that intercept TLS
    cp "$EXTRA_CA" /etc/ca-certificates/trust-source/anchors/extra.crt && update-ca-trust
fi
pacman -Syu --noconfirm --needed archiso rust python python-pip grub >/dev/null

echo "==> building linuxai-core"
export CARGO_TARGET_DIR="$WORK/target" CARGO_HTTP_CAINFO=/etc/ssl/certs/ca-certificates.crt
cargo build --release --locked --manifest-path "$SRC/core/Cargo.toml"

echo "==> assembling profile"
PROFILE="$WORK/profile"
rm -rf "$PROFILE" && cp -r /usr/share/archiso/configs/releng "$PROFILE"
cp -a "$SRC/image/airootfs/." "$PROFILE/airootfs/"
grep -v '^#' "$SRC/image/packages.extra" >> "$PROFILE/packages.x86_64"
install -Dm755 "$WORK/target/release/linuxai-core" "$PROFILE/airootfs/usr/local/bin/linuxai-core"

echo "==> prompt library"
mkdir -p "$PROFILE/airootfs/home/linuxai/prompts"
cp -r "$SRC/library/prompts/." "$PROFILE/airootfs/home/linuxai/prompts/"
mkdir -p "$PROFILE/airootfs/usr/share/linuxai/image-backends"
cp -r "$SRC/library/image-backends/." "$PROFILE/airootfs/usr/share/linuxai/image-backends/"

echo "==> installing the LinuxAI AI layer (python) into /opt/linuxai"
rm -rf "$WORK/pysrc" && mkdir -p "$WORK/pysrc" && cp -r "$SRC/pyproject.toml" "$SRC/README.md" "$SRC/linuxai" "$WORK/pysrc/"
python -m pip install --quiet --no-compile --break-system-packages --cert /etc/ssl/certs/ca-certificates.crt \
    --target "$PROFILE/airootfs/opt/linuxai/lib" "$WORK/pysrc"

# The human's account: logs straight into LinuxAI on tty1.
echo 'linuxai:x:1000:1000:LinuxAI:/home/linuxai:/bin/bash' >> "$PROFILE/airootfs/etc/passwd"
echo 'linuxai::14871::::::'                                 >> "$PROFILE/airootfs/etc/shadow"
grep -q '^wheel:' "$PROFILE/airootfs/etc/group" 2>/dev/null || echo 'wheel:x:998:linuxai' >> "$PROFILE/airootfs/etc/group"
echo 'linuxai:x:1000:'                                      >> "$PROFILE/airootfs/etc/group"
[ -f "$PROFILE/airootfs/etc/gshadow" ] || echo 'root:!*::root' > "$PROFILE/airootfs/etc/gshadow"
echo 'linuxai:!*::'                                         >> "$PROFILE/airootfs/etc/gshadow"

# Identity and permissions.
sed -i -e "s/^iso_name=.*/iso_name=\"linuxai\"/" \
       -e "s/^iso_label=.*/iso_label=\"LINUXAI_$(date +%Y%m)\"/" \
       -e "s/^iso_publisher=.*/iso_publisher=\"LinuxAI\"/" \
       -e "s/^iso_application=.*/iso_application=\"LinuxAI live\"/" \
       -e "s/^iso_version=.*/iso_version=\"$VERSION\"/" "$PROFILE/profiledef.sh"
sed -i '/^file_permissions=(/a\  ["/etc/gshadow"]="0:0:400"\n  ["/etc/sudoers.d/10-linuxai"]="0:0:440"\n  ["/home/linuxai"]="1000:1000:750"\n  ["/home/linuxai/.bash_profile"]="1000:1000:644"\n  ["/usr/local/bin/linuxai-core"]="0:0:755"\n  ["/usr/local/bin/cortex"]="0:0:755"\n  ["/usr/local/bin/linuxai-session"]="0:0:755"' "$PROFILE/profiledef.sh"

# Also show boot and login on the serial port (handy for VMs and automated tests).
grep -rl 'archisobasedir' "$PROFILE/efiboot" "$PROFILE/syslinux" "$PROFILE/grub" 2>/dev/null \
    | xargs sed -i '/archisobasedir/ s/$/ console=tty0 console=ttyS0,115200/'
# Rename the boot menu entries.
grep -rl 'Arch Linux install medium' "$PROFILE" 2>/dev/null | xargs -r sed -i 's/Arch Linux install medium/LinuxAI live/g'

echo "==> mkarchiso"
mkdir -p "$OUT"
mkarchiso -v -r -w "$WORK/mkarchiso" -o "$OUT" "$PROFILE"
ls -lh "$OUT"

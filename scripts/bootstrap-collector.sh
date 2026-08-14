#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Declaratively erase a disk and install the collector appliance — the
complete install in one command, run on the NixOS installer (normally inside
an SSH session from your machine after setting a temporary root password;
the only files to copy over are your SSH public key and, optionally, the
device credentials file).

  scp ~/.ssh/collector-ops.pub root@INSTALLER_IP:/tmp/ops.pub
  scp credentials.json root@INSTALLER_IP:/tmp/credentials.json   # optional
  ssh root@INSTALLER_IP
  nix run github:chris-arsenault/ahara-collector#bootstrap-collector -- \
    --disk /dev/disk/by-id/ID --key-file /tmp/ops.pub \
    --credentials-file /tmp/credentials.json

Topology and service configuration come from the versioned release. Bootstrap
writes only hardware and access identity to this machine; the pull-deploy
updater overlays those machine values on every build.

Required:
  --disk PATH             Stable whole-disk /dev/disk/by-id path to erase.
  --key-file PATH         SSH public key(s), one per line — authorized for
                          the ops user from first boot.

Optional:
  --interface IFACE       Override NIC discovery (default: the ethernet
                          port with link, or the only ethernet port).
  --credentials-file PATH Device credentials JSON to seed
                          /var/lib/ahara-collector/credentials.json
                          ({"envSensors": {"username": ..., "password": ...},
                            "kasa": {...}}). Omit to upload via scp later.

Safety:
  --confirm-disk PATH     Non-interactive confirmation; must equal --disk.
  --dry-run               Discover, render, validate, and show the disko
                          plan without touching the disk.
EOF
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

disk=
key_file=
interface=
credentials_file=
confirm_disk=
dry_run=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --disk) disk=$2; shift 2 ;;
    --key-file) key_file=$2; shift 2 ;;
    --interface) interface=$2; shift 2 ;;
    --credentials-file) credentials_file=$2; shift 2 ;;
    --confirm-disk) confirm_disk=$2; shift 2 ;;
    --dry-run) dry_run=true; shift ;;
    -h | --help) usage; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done

[[ ${EUID} -eq 0 ]] || die "run this command as root from the NixOS installer"
[[ -n "$disk" ]] || die "--disk is required"
[[ "$disk" == /dev/disk/by-id/* ]] || die "--disk must use a stable /dev/disk/by-id path"
[[ "$disk" != *-part[0-9]* ]] || die "--disk must identify a whole disk, not a partition"
[[ -e "$disk" ]] || die "$disk does not exist"
[[ -n "$key_file" ]] || die "--key-file is required (scp your public key to the installer first)"
[[ -r "$key_file" ]] || die "cannot read $key_file"
if [[ -n "$credentials_file" ]]; then
  [[ -r "$credentials_file" ]] || die "cannot read $credentials_file"
fi
[[ "$(uname -m)" == x86_64 ]] || die "the collector configuration is x86_64 only"
[[ -d /sys/firmware/efi/efivars ]] || die "the installer must be booted through UEFI (systemd-boot)"
grep -q '^ID=nixos' /etc/os-release 2>/dev/null || die "run this from the NixOS installer"
! findmnt -S "$disk" >/dev/null 2>&1 || die "$disk has mounted filesystems; unmount first"

flake="${COLLECTOR_BOOTSTRAP_FLAKE:?COLLECTOR_BOOTSTRAP_FLAKE is not set}"
export NIX_CONFIG="${NIX_CONFIG:-}
experimental-features = nix-command flakes"

# ---- admin keys -------------------------------------------------------------

admin_keys=""
while IFS= read -r line; do
  [ -z "$line" ] && continue
  case "$line" in
    ssh-* | sk-ssh-*) admin_keys+="$line"$'\n' ;;
    *) die "not an SSH public key line: $line" ;;
  esac
done <"$key_file"
[ -n "$admin_keys" ] || die "$key_file contains no SSH public keys"

# ---- discover the NIC -------------------------------------------------------

if [ -z "$interface" ]; then
  candidates=()
  for path in /sys/class/net/*; do
    name=$(basename "$path")
    [ "$name" = lo ] && continue
    [ -e "$path/device" ] || continue # skip virtual interfaces
    [ "$(cat "$path/type")" = 1 ] || continue # ethernet only
    candidates+=("$name")
  done
  [ ${#candidates[@]} -gt 0 ] || die "no ethernet interface found; pass --interface"
  interface=""
  for name in "${candidates[@]}"; do
    if [ "$(cat "/sys/class/net/$name/carrier" 2>/dev/null || echo 0)" = 1 ]; then
      interface=$name
      break
    fi
  done
  if [ -z "$interface" ]; then
    [ ${#candidates[@]} -eq 1 ] || die "multiple ethernet ports and none has link; pass --interface"
    interface=${candidates[0]}
  fi
fi
[ -e "/sys/class/net/$interface" ] || die "no such interface: $interface"
interface_mac=$(cat "/sys/class/net/$interface/address")
echo "using interface $interface ($interface_mac)"

# ---- render and validate this machine's identity ----------------------------

workdir=$(mktemp -d /tmp/bootstrap-collector.XXXXXX)
trap 'rm -rf "$workdir"' EXIT

INTERFACE_MAC="$interface_mac" \
ADMIN_KEYS="$admin_keys" \
  bash "${COLLECTOR_RENDER_MACHINE:?COLLECTOR_RENDER_MACHINE is not set}" >"$workdir/machine-values.json"

echo "rendered machine values:"
sed 's/^/  /' "$workdir/machine-values.json"

cp -rT "$flake" "$workdir/repo"
chmod -R u+w "$workdir/repo"
install -m 0644 "$workdir/machine-values.json" "$workdir/repo/hosts/collector/machine-values.json"

echo "validating versioned topology with rendered machine values..."
nix build --dry-run "path:$workdir/repo#nixosConfigurations.collector.config.system.build.toplevel" \
  || die "rendered values failed validation; nothing was touched"

# ---- partition and mount ----------------------------------------------------

disko="${COLLECTOR_DISKO:?COLLECTOR_DISKO is not set}"
if [ "$dry_run" = true ]; then
  echo "dry run: skipping disko and install. The disk would be erased with:"
  echo "  $disko --mode destroy,format,mount --flake path:$workdir/repo#collector --argstr disk $disk"
  exit 0
fi

if [ -n "$confirm_disk" ]; then
  [ "$confirm_disk" = "$disk" ] || die "--confirm-disk does not match --disk"
  yes_flag=--yes-wipe-all-disks
else
  echo "about to ERASE $disk. Type: erase $disk"
  read -r answer
  [ "$answer" = "erase $disk" ] || die "confirmation did not match"
  yes_flag=--yes-wipe-all-disks
fi

"$disko" --mode destroy,format,mount $yes_flag \
  --flake "path:$workdir/repo#collector" --argstr disk "$disk"

findmnt /mnt >/dev/null || die "disko did not mount the root filesystem"
findmnt /mnt/boot >/dev/null || die "disko did not mount the ESP"

# ---- seed host state and install --------------------------------------------

install -D -m 0644 "$workdir/machine-values.json" /mnt/var/lib/ahara-collector/machine-values.json
if [ -n "$credentials_file" ]; then
  install -D -m 0600 "$credentials_file" /mnt/var/lib/ahara-collector/credentials.json
fi

nixos-install --no-root-passwd --flake "path:$workdir/repo#collector"

address=$(nix eval --raw --impure --expr \
  "(builtins.fromJSON (builtins.readFile \"$workdir/repo/hosts/collector/topology.json\")).network.address")

cat <<EOF

Installed. Next steps:
  1. Reboot into the installed system and SSH in as ops@$address.
  2. Read the House Sensors bearer token:
       sudo cat /var/lib/ahara-collector/api-token
  3. Read the separately scoped Airwave bearer token:
       sudo cat /var/lib/ahara-collector/airwave-token
EOF
if [ -z "$credentials_file" ]; then
  cat <<'EOF'
  4. Upload the device credentials, then restart the collector:
       scp credentials.json ops@ADDRESS:/tmp/credentials.json
       ssh ops@ADDRESS 'sudo install -m 0600 -o root -g root \
         /tmp/credentials.json /var/lib/ahara-collector/credentials.json \
         && rm /tmp/credentials.json && sudo systemctl restart ahara-collector'
EOF
fi
echo "Machine identity stays on this host. Topology changes move through Git."

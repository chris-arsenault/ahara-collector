#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Declaratively erase a disk and install the S13 collector appliance — the
complete install in one command, run on the NixOS installer (normally inside
an SSH session from your machine after setting a temporary root password;
the only files to copy over are your SSH public key and, optionally, the
device credentials file).

  scp ~/.ssh/s13-ops.pub root@INSTALLER_IP:/tmp/ops.pub
  scp credentials.json root@INSTALLER_IP:/tmp/credentials.json   # optional
  ssh root@INSTALLER_IP
  nix run github:chris-arsenault/ahara-collector#bootstrap-s13 -- \
    --disk /dev/disk/by-id/ID --key-file /tmp/ops.pub \
    --address 192.168.65.3 --home-lan-cidr 192.168.65.0/24 \
    --router-ip 192.168.65.1 \
    --credentials-file /tmp/credentials.json

The values become this machine's configuration store — nothing is committed
to git. The repo's committed values stay placeholders; the pull-deploy
updater overlays this machine's values on every build.

Required:
  --disk PATH             Stable whole-disk /dev/disk/by-id path to erase.
  --key-file PATH         SSH public key(s), one per line — authorized for
                          the ops user from first boot.
  --address IP            This appliance's static home-LAN address (pick one
                          outside the router's DHCP pool).
  --home-lan-cidr CIDR    The home LAN.
  --router-ip IP          Router's address on that home LAN.

Optional:
  --truenas-ip IP         TrueNAS address on the server subnet
                          (default: 192.168.66.3). Airwave SSDP ingress and
                          the readings pull are pinned to it.
  --dns IP                DNS server; may repeat (default: 9.9.9.9 and
                          149.112.112.112).
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
address=
home_lan_cidr=
router_ip=
truenas_ip=192.168.66.3
interface=
credentials_file=
confirm_disk=
dry_run=false
dns_servers=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --disk) disk=$2; shift 2 ;;
    --key-file) key_file=$2; shift 2 ;;
    --address) address=$2; shift 2 ;;
    --home-lan-cidr) home_lan_cidr=$2; shift 2 ;;
    --router-ip) router_ip=$2; shift 2 ;;
    --truenas-ip) truenas_ip=$2; shift 2 ;;
    --dns) dns_servers+=("$2"); shift 2 ;;
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
[[ -n "$address" ]] || die "--address is required"
[[ -n "$home_lan_cidr" ]] || die "--home-lan-cidr is required"
[[ -n "$router_ip" ]] || die "--router-ip is required"
if [[ -n "$credentials_file" ]]; then
  [[ -r "$credentials_file" ]] || die "cannot read $credentials_file"
fi
[[ "$(uname -m)" == x86_64 ]] || die "the S13 configuration is x86_64 only"
[[ -d /sys/firmware/efi/efivars ]] || die "the installer must be booted through UEFI (systemd-boot)"
grep -q '^ID=nixos' /etc/os-release 2>/dev/null || die "run this from the NixOS installer"
! findmnt -S "$disk" >/dev/null 2>&1 || die "$disk has mounted filesystems; unmount first"

[[ ${#dns_servers[@]} -gt 0 ]] || dns_servers=(9.9.9.9 149.112.112.112)

flake="${S13_BOOTSTRAP_FLAKE:?S13_BOOTSTRAP_FLAKE is not set}"
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

# ---- render and validate this machine's values ------------------------------

workdir=$(mktemp -d /tmp/bootstrap-s13.XXXXXX)
trap 'rm -rf "$workdir"' EXIT

INTERFACE_MAC="$interface_mac" \
HOME_LAN_CIDR="$home_lan_cidr" \
ADDRESS="$address" \
ROUTER_IP="$router_ip" \
TRUENAS_IP="$truenas_ip" \
DNS_SERVERS="$(printf '%s\n' "${dns_servers[@]}")" \
ADMIN_KEYS="$admin_keys" \
  bash "${S13_RENDER:?S13_RENDER is not set}" >"$workdir/site-values.json"

echo "rendered site values:"
sed 's/^/  /' "$workdir/site-values.json"

cp -rT "$flake" "$workdir/repo"
chmod -R u+w "$workdir/repo"
install -m 0644 "$workdir/site-values.json" "$workdir/repo/hosts/s13/site-values.json"

echo "validating rendered values..."
nix build --dry-run "path:$workdir/repo#nixosConfigurations.s13.config.system.build.toplevel" \
  || die "rendered values failed validation; nothing was touched"

# ---- partition and mount ----------------------------------------------------

disko="${S13_DISKO:?S13_DISKO is not set}"
if [ "$dry_run" = true ]; then
  echo "dry run: skipping disko and install. The disk would be erased with:"
  echo "  $disko --mode destroy,format,mount --flake path:$workdir/repo#s13 --argstr disk $disk"
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
  --flake "path:$workdir/repo#s13" --argstr disk "$disk"

findmnt /mnt >/dev/null || die "disko did not mount the root filesystem"
findmnt /mnt/boot >/dev/null || die "disko did not mount the ESP"

# ---- seed host state and install --------------------------------------------

install -D -m 0644 "$workdir/site-values.json" /mnt/var/lib/ahara-collector/site-values.json
if [ -n "$credentials_file" ]; then
  install -D -m 0600 "$credentials_file" /mnt/var/lib/ahara-collector/credentials.json
fi

nixos-install --no-root-passwd --flake "path:$workdir/repo#s13"

cat <<EOF

Installed. Next steps:
  1. Reboot into the installed system and SSH in as ops@$address.
  2. Read the API bearer token for the TrueNAS pull job:
       sudo cat /var/lib/ahara-collector/api-token
EOF
if [ -z "$credentials_file" ]; then
  cat <<'EOF'
  3. Upload the device credentials, then restart the collector:
       scp credentials.json ops@ADDRESS:/tmp/credentials.json
       ssh ops@ADDRESS 'sudo install -m 0600 -o root -g root \
         /tmp/credentials.json /var/lib/ahara-collector/credentials.json \
         && rm /tmp/credentials.json && sudo systemctl restart ahara-collector'
EOF
fi
echo "Nothing to commit: this machine's values live only on this machine."

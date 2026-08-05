#!/usr/bin/env bash
set -euo pipefail
# Renders this machine's configuration store (site-values.json) to stdout
# from environment variables. Used by bootstrap-s13 to seed the store at
# install time; afterwards the store is edited in place on the host (rare)
# and overlaid by the updater on every build.
#
# Required: INTERFACE_MAC HOME_LAN_CIDR ADDRESS ROUTER_IP TRUENAS_IP
# Newline-separated lists: DNS_SERVERS ADMIN_KEYS

for v in INTERFACE_MAC HOME_LAN_CIDR ADDRESS ROUTER_IP TRUENAS_IP \
  DNS_SERVERS ADMIN_KEYS; do
  [ -n "${!v:-}" ] || { printf 'render-site-values: %s is not set\n' "$v" >&2; exit 1; }
done

# JSON strings: escape backslashes and quotes. Most values are MACs and
# addresses, but an SSH key's trailing comment can contain anything.
json_string() {
  local s=$1
  s=${s//\\/\\\\}
  s=${s//\"/\\\"}
  printf '"%s"' "$s"
}

# Newline-separated input -> indented, comma-separated JSON array elements.
json_list() {
  local indent=$1 first=1 item
  while IFS= read -r item; do
    [ -n "$item" ] || continue
    [ "$first" -eq 1 ] || printf ',\n'
    printf '%s%s' "$indent" "$(json_string "$item")"
    first=0
  done <<<"$2"
  [ "$first" -eq 1 ] || printf '\n'
}

# Module and deployment sections start at the committed defaults; they are
# host state and can be edited on the machine later.
cat <<EOF
{
  "_comment": "Configuration store, GENERATED $(date -u +%Y-%m-%dT%H:%M:%SZ) by bootstrap-s13. Edit on the host at /var/lib/ahara-collector/site-values.json; the updater overlays it on every build.",
  "hostName": "s13",
  "stateVersion": "26.05",
  "adminAuthorizedKeys": [
$(json_list "    " "$ADMIN_KEYS")  ],
  "network": {
    "interfaceMac": $(json_string "$INTERFACE_MAC"),
    "homeLanCidr": $(json_string "$HOME_LAN_CIDR"),
    "address": $(json_string "$ADDRESS"),
    "routerIp": $(json_string "$ROUTER_IP"),
    "dnsServers": [
$(json_list "      " "$DNS_SERVERS")    ],
    "truenasIp": $(json_string "$TRUENAS_IP")
  },
  "deployment": {
    "repoUrl": "https://github.com/chris-arsenault/ahara-collector",
    "branch": "release",
    "pollIntervalMinutes": 2
  },
  "api": {
    "port": 8850
  },
  "airwaveSsdp": {
    "enable": true,
    "relayPort": 1901,
    "responseWindowSeconds": 4
  },
  "envSensors": {
    "enable": true,
    "discoveryPort": 12343,
    "devicePort": 80,
    "pollIntervalSeconds": 1,
    "discoveryIntervalHours": 4
  },
  "kasa": {
    "enable": true,
    "discoveryPort": 20002,
    "devicePort": 80,
    "pollIntervalSeconds": 1,
    "discoveryIntervalHours": 4,
    "staticDevices": []
  },
  "spool": {
    "maxBytes": 268435456,
    "segmentBytes": 4194304
  }
}
EOF

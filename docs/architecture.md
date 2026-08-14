# Architecture

The collector is a single-purpose appliance on the IoT LAN: it is the
one host that faces the house's IoT devices, and the only surface it offers
the rest of the network is one authenticated TCP port, served over TLS at
`collector.local.ahara.io`.

## Topology

```text
UniFi gateway
   ├── Home LAN (192.168.65.0/24)
   ├── IoT LAN (192.168.30.0/24; router .1)
   │      ├── WiiM players (SSDP/UPnP)
   │      ├── AtomS3U env sensors (HTTP + UDP discovery)
   │      ├── Kasa KP125M plugs (KLAP)
   │      └── collector (192.168.30.2)
   └── Uplink VLAN (192.168.60.0/24) ── VP2440 (.60.2)
                                               └── Server subnet ── TrueNAS (Airwave, InfluxDB, pull job)
```

The appliance has one interface, renamed `lan0` by permanent MAC, with a
static address chosen outside the router's DHCP pool. TrueNAS traffic
reaches it routed through the VP2440 with original source addresses (the
gateway does no NAT), so the local firewall pins server-side flows to the
TrueNAS address. The gateway-side flow declarations live in ahara-vpn;
[integration.md](integration.md) specifies them.

## Single source of truth

`hosts/collector/site.nix` composes versioned `topology.json` with the host's
`machine-values.json` (ADR-0009). Topology owns addresses, ports, deployment,
module settings, and spool limits. Machine values own only the interface MAC
and administrator keys; the updater overlays them on every build.
`lib/site-assertions.nix` fails evaluation on missing or inconsistent
values — placeholder and real alike. The Rust service receives its topology
as one JSON document rendered from the site; it contains no addresses of
its own.

## The collector service

One Rust binary (`service/`), built from a locked dependency closure and run
as a hardened `DynamicUser` systemd unit, has four concerns:

**Airwave SSDP relay** (ADR-0001). Airwave sends M-SEARCH (from its fixed
response port 1901) and MediaServer NOTIFYs to the collector's IoT-LAN
address, port 1900. The relay validates each message and re-originates it
on-link — multicast 239.255.255.250 plus the directed broadcast, because
IoT Wi-Fi has been observed suppressing multicast delivery. Renderer
replies within the search window (MX-derived, bounded) are validated
(MediaRenderer ST, LOCATION inside the IoT subnet) and returned to
Airwave's port 1901. WiiM-originated M-SEARCH for MediaServer targets is
relayed to Airwave and its unicast answers are returned to the requesting
device, so both discovery directions work across the subnet split.

**WiiM inventory.** A separate on-link discovery socket originates
MediaRenderer searches from the IoT address, validates every response and
description endpoint against the IoT CIDR, and parses the device's native UDN,
identity, and advertised service control paths. The current snapshot is
available from `/devices`; last-known entries persist as appliance runtime
state and are marked unreachable until a later scan sees them. This inventory
has no reading spool and carries no Airwave playback or grouping state.

**Device pollers.** The environment-sensor module discovers AtomS3U
devices by UDP broadcast, validates them against `/sensors`, and polls at
1 Hz with the shared Basic credentials; the Kasa module (experimental,
ADR-0005) discovers KP125M plugs on UDP 20002 and reads energy usage over
KLAP sessions. Both emit device-native reading envelopes — the device's
own payload verbatim, wrapped with module, device identity, and a
timestamp — and never a measurement, field, or bucket name: the data
schema is owned entirely by house-sensors (ADR-0006;
[integration.md](integration.md) specifies the envelope). Credentials are
host state (ADR-0003); a module without credentials idles.

**Spools and API** (ADR-0002, ADR-0007). Each module's readings append to
that module's own bounded on-disk spool (size-capped, oldest-dropped,
crash-tolerant) and wait for that module's consumer to pull them — one
stream per consumer, no fan-out anywhere. The API is one port:

| Route | Auth | Purpose |
| ----- | ---- | ------- |
| `GET /health` | none | liveness for the deploy gate and consumers |
| `GET /metrics` | bearer | service counters plus host load/memory gauges |
| `GET /devices` | bearer | discovered environment, Kasa, and WiiM devices |
| `GET /readings/next?module=<name>` | bearer | oldest closed segment of that module's spool |
| `POST /readings/ack` | bearer | delete a drained batch (`module` + `batchId`) |
| `POST /ingest` | device Basic | device-originated envelope push, routed by each envelope's module |

The bearer token is generated on the host at first boot and read by the
TrueNAS pull job's operator once. Host metrics are served from `/proc` on
the same port, so the appliance needs no node-exporter and no second
listener.

nginx fronts that port with TLS for `collector.local.ahara.io` (ADR-0008),
so the token and the readings cross the gateway path encrypted and the
service's own plaintext connection never leaves this host. The certificate is
publicly trusted and fetched from the machine-identity appliance, which the
collector authenticates to with the identity it enrolled for; this appliance
holds no cloud credential of its own. Without a certificate nginx does not
start, so an appliance that cannot obtain one fails its deploy rather than
serving a placeholder nobody would notice. Expiry is exported as a textfile
metric.

## Firewall

The NixOS nftables firewall defaults to drop and opens exactly the declared
surface: SSH from the trusted home LAN; the API's TLS and plain ports from
TrueNAS, the home LAN, and the IoT LAN; SSDP 1900 from TrueNAS and on-link
devices; the relay reply port; the WiiM inventory reply port; and the two
fixed sensor discovery-reply ports (broadcast requests cannot ride
conntrack, so the service binds fixed source ports and the rules stay
narrow). Every rule carries a `collector:` comment.

## Deployment

The ahara-vpn pull pattern (ADR-0004): CI advances `release`; the
`collector-update` timer polls it, overlays machine values, builds, activates,
and commits the generation only when `collector-health-check` passes; failures
roll back. First install is one `bootstrap-collector` command on the NixOS
installer ([runbook](runbook.md)).

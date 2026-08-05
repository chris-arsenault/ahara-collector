# Architecture

The S13 is a single-purpose collector appliance on the home LAN: it is the
one host that faces the house's IoT devices, and the only surface it offers
the rest of the network is one authenticated TCP port.

## Topology

```text
Existing router (192.168.65.1) ─── Home LAN (HOME_LAN_CIDR)
   │                                 ├── WiiM players (SSDP/UPnP)
   │                                 ├── AtomS3U env sensors (HTTP + UDP discovery)
   │                                 ├── Kasa KP125M plugs (KLAP)
   │                                 └── S13 collector (this host, static address)
   └── VP2440 gateway ─── Server subnet ─── TrueNAS (Airwave, InfluxDB, pull job)
```

The appliance has one interface, renamed `lan0` by permanent MAC, with a
static address chosen outside the router's DHCP pool. TrueNAS traffic
reaches it routed through the VP2440 with original source addresses (the
gateway does no NAT), so the local firewall pins server-side flows to the
TrueNAS address. The gateway-side flow declarations live in ahara-vpn;
[integration.md](integration.md) specifies them.

## Single source of truth

`hosts/s13/site.nix` declares every address, port, and module setting,
deriving machine-specific inputs from `site-values.json`. The repo commits
placeholder values (what CI and the VM test build); a real machine's values
are host state at `/var/lib/ahara-collector/site-values.json`, rendered by
the bootstrap installer and overlaid by the updater on every build.
`lib/site-assertions.nix` fails evaluation on missing or inconsistent
values — placeholder and real alike. The Rust service receives its topology
as one JSON document rendered from the site; it contains no addresses of
its own.

## The collector service

One dependency-free Rust binary (`service/`), run as a hardened
`DynamicUser` systemd unit with three concerns:

**Airwave SSDP relay** (ADR-0001). Airwave sends M-SEARCH (from its fixed
response port 1901) and MediaServer NOTIFYs to the collector's home
address, port 1900. The relay validates each message and re-originates it
on-link — multicast 239.255.255.250 plus the directed broadcast, because
home Wi-Fi has been observed suppressing multicast delivery. Renderer
replies within the search window (MX-derived, bounded) are validated
(MediaRenderer ST, LOCATION inside the home subnet) and returned to
Airwave's port 1901. WiiM-originated M-SEARCH for MediaServer targets is
relayed to Airwave and its unicast answers are returned to the requesting
device, so both discovery directions work across the subnet split.

**Device pollers.** The environment-sensor module discovers AtomS3U
devices by UDP broadcast, validates them against `/sensors`, and polls at
1 Hz with the shared Basic credentials; the Kasa module (experimental,
ADR-0005) discovers KP125M plugs on UDP 20002 and reads energy usage over
KLAP sessions. Credentials are host state (ADR-0003); a module without
credentials idles.

This repo owns the reading schema. Two measurements, one field per
quantity, units normalized at the edge, time only in the line timestamp
(corrected by the device-reported sample age where available):

| Measurement | Tags | Fields |
| ----------- | ---- | ------ |
| `environment` | `device`, `ip`, `model`, user tags | `temperature_c`, `humidity`, `pressure_pa`, `sample_age_ms` |
| `power` | `device`, `ip`, `model` | `power_w`, `voltage_v`, `current_a`, `energy_today_kwh` |

**Spool and API** (ADR-0002). Readings append to a bounded on-disk spool
(size-capped, oldest-dropped, crash-tolerant) and wait for TrueNAS to pull
them. The API is one port:

| Route | Auth | Purpose |
| ----- | ---- | ------- |
| `GET /health` | none | liveness for the deploy gate and pull job |
| `GET /metrics` | bearer | service counters plus host load/memory gauges |
| `GET /devices` | bearer | discovered devices per module |
| `GET /readings/next` | bearer | oldest closed spool segment as a batch |
| `POST /readings/ack` | bearer | delete a drained batch |
| `POST /ingest` | device Basic | line-protocol push path for future firmware |

The bearer token is generated on the host at first boot and read by the
TrueNAS pull job's operator once. Host metrics are served from `/proc` on
the same port, so the appliance needs no node-exporter and no second
listener.

## Firewall

The NixOS nftables firewall defaults to drop and opens exactly the
declared surface: SSH from the home LAN, the API port from TrueNAS and the
home LAN, SSDP 1900 (TrueNAS unicast plus on-link), the relay reply port,
and the two fixed discovery-reply ports (broadcast requests cannot ride
conntrack, so the service binds fixed source ports and the rules stay
narrow). Every rule carries a `collector:` comment.

## Deployment

The ahara-vpn pull pattern, unchanged (ADR-0004): CI advances `release`;
the `s13-update` timer polls it, overlays host values, builds, activates,
and commits the generation only when `s13-health-check` passes; failures
roll back. First install is one `bootstrap-s13` command on the NixOS
installer ([runbook](runbook.md)).

# Changelog

All notable user-visible changes are recorded here.

## Unreleased

### Appliance

- Static topology and service settings now live in versioned `topology.json`;
  only interface identity and administrator keys remain in machine-local
  `machine-values.json`. Existing combined stores migrate automatically and
  are archived, and bootstrap no longer duplicates topology as command flags.
- The collector moves from the home LAN to `192.168.30.2` on the dedicated
  IoT LAN. Its gateway, internal DNS resolver, firewall source network, SSDP
  broadcast, sensor discovery, tests, and operator examples move with it;
  identity remains bound to `collector.local.ahara.io`, and DNS is served by
  the gateway at `192.168.60.2`.
- Everything is named for what it does rather than what it is:
  `hosts/collector`, `nixosConfigurations.collector`, the `collector-update`
  and `collector-health-check` units, `bootstrap-collector`, and the hostname
  `collector`. Host state was already at `/var/lib/ahara-collector`, so
  nothing moves on disk. The hardware is still a Beelink S13 and the docs
  still say so where the physical machine is meant.
- The pull API is served over TLS at `collector.local.ahara.io:8443` with a
  publicly-trusted certificate the machine-identity appliance distributes, so
  the bearer token and readings no longer cross the gateway path in plaintext.
  This appliance generates no stand-in and holds no cloud credential of any
  kind. The plain port stays open until the
  TrueNAS puller cuts over. The deploy health gate checks that the
  terminator answers, so a release that cannot serve TLS rolls back.
- The shared-certificate client includes its file-comparison utility, allowing
  renewals to compare and replace an existing certificate unattended, and
  queues the nginx reload without creating a systemd ordering deadlock.
- The S13 collector appliance exists: a NixOS host on the IoT LAN with a
  one-command bootstrap installer, pull-based self-deployment gated by
  health checks with rollback, and a default-drop firewall opening exactly
  its declared surface.
- Airwave registers its UPnP MediaServer through the collector API; the
  collector answers WiiM searches and advertises the lease on the IoT LAN.
  Cross-VLAN SSDP forwarding and its response socket are removed.
- A separate WiiM inventory module discovers MediaRenderers locally,
  validates descriptions and service endpoints against the IoT CIDR, exposes
  their native identities and control paths through `/wiim/devices`, and persists
  last-known addresses as runtime state without creating a readings stream.
- Airwave now has a separately authenticated WiiM API: registry-constrained
  UPnP and LinkPlay transport, grouped-renderer probing, and renewable local
  MediaServer advertisement. Device IDs and advertised service paths choose
  every outbound destination; redirects and oversized responses are rejected.
- Outbound device HTTP/TLS and XML parsing use a locked dependency closure so
  later WiiM control proxying can remain in testable Rust code while nginx
  continues to terminate only the collector API's inbound TLS.
- Environment sensors are discovered and polled from the collector with
  the shared device credentials; Kasa KP125M polling over KLAP is
  implemented and marked experimental pending hardware validation. Both
  modules emit device-native reading envelopes — the data schema is owned
  entirely by house-sensors.
- Readings buffer in a bounded on-disk spool per module (oldest-dropped,
  crash tolerant), and each house-sensors collector drains its own
  module's stream through a single authenticated API port with
  at-least-once batch delivery; the same port serves health, metrics
  (including host gauges), device listings, and a Basic-auth device push path.
- Device credentials are one root-owned host-state file, seedable at
  bootstrap or by scp, handed to the sandboxed service via systemd
  credentials; modules without credentials idle instead of failing.

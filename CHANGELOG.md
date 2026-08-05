# Changelog

All notable user-visible changes are recorded here.

## Unreleased

### Appliance

- The S13 collector appliance exists: a NixOS host on the home LAN with a
  one-command bootstrap installer, pull-based self-deployment gated by
  health checks with rollback, and a default-drop firewall opening exactly
  its declared surface.
- Airwave SSDP is relayed natively on-link: M-SEARCH and MediaServer
  announcements re-originate from the collector's home-LAN address
  (multicast plus directed broadcast), renderer replies return to Airwave's
  fixed response port within bounded search windows, and WiiM-originated
  MediaServer searches are answered across the subnet split. This replaces
  the gateway-hosted relay attempt, whose off-subnet SSDP the WiiM devices
  ignored.
- Environment sensors are discovered and polled from the collector with
  the shared device credentials, producing byte-compatible `environment`
  lines; Kasa KP125M polling over KLAP is implemented and marked
  experimental pending hardware validation.
- Readings buffer in a bounded on-disk spool (oldest-dropped, crash
  tolerant) and are drained by TrueNAS through a single authenticated API
  port with at-least-once batch delivery; the same port serves health,
  metrics (including host gauges), device listings, and a Basic-auth push
  path for future firmware.
- Device credentials are one root-owned host-state file, seedable at
  bootstrap or by scp, handed to the sandboxed service via systemd
  credentials; modules without credentials idle instead of failing.

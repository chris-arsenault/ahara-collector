# 0001 — The collector is a dedicated appliance on the home LAN

- Status: Accepted
- Date: 2026-08-05

## Context

Airwave (on TrueNAS, server subnet) must discover WiiM players on the home
LAN. ahara-vpn tried two shapes: directed-broadcast SSDP forwarded by the
gateway (its ADR-0012), which the WiiMs ignored because the source address
was off their subnet, and a relay process hosted on the gateway itself,
which worked but made the default-drop routing appliance terminate device
traffic, forced an input-chain path into its firewall generator, and left
the relay unable to hear multicast. The house-sensors pollers on TrueNAS
had the mirrored problem — reaching home-LAN devices only through
gateway-forwarded broadcast flows, device credentials on the same host as
every other service — and stopped working entirely at the subnet split;
they are defunct, not migrated from.

## Decision

Device-facing work moves to a dedicated NixOS appliance (Beelink Mini S13)
with a single interface on the home LAN. It speaks native on-link SSDP —
multicast joined, directed broadcast as the Wi-Fi fallback — so discovery
originates from an address the WiiMs accept. It holds the IoT device
credentials, polls the sensors, and buffers readings locally. Traffic
between TrueNAS and the appliance crosses the VP2440 as ordinary declared,
inspectable forward flows; the gateway goes back to being only a router.

## Alternatives considered

- **Keep the relay on the gateway.** Works for SSDP replies but never for
  multicast listening, and every module added to the gateway widens the
  appliance whose entire design is minimal attack surface.
- **Host the collector on TrueNAS with a home-LAN VLAN leg.** Gives the
  busiest host a foot in both networks and couples device credentials to
  the service host the collector is meant to shield.

## Consequences

- Collector failure stops WiiM discovery and sensor collection, but no
  routed data path: the appliance is not in line for anything.
- The gateway needs three declared flows for the appliance
  (docs/integration.md); the directed-broadcast Airwave flows and the
  TrueNAS→IoT polling flows served the defunct pollers and can simply be
  removed.
- A future gateway-served IoT VLAN is a one-value re-address of this
  appliance, not a topology change.

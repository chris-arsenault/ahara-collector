# 0012 — The collector resides on the dedicated IoT LAN

- Status: Accepted
- Date: 2026-08-16
- Supersedes: ADR-0001

## Context

ADR-0001 placed the collector on the home LAN because the first implementation
needed layer-2 discovery and polling beside the devices. The devices now live
on a dedicated IoT VLAN. Relaying each discovery protocol to a collector on the
home LAN would restore broad cross-VLAN traffic and make the gateway responsible
for device-specific multicast and broadcast behavior.

The collector already forms the narrow boundary between device protocols and
their consumers. It can preserve that boundary by moving with the devices and
exposing only its authenticated API through routed firewall flows.

## Decision

The collector uses one interface on the IoT LAN at the versioned topology
address. Discovery, polling, WiiM inventory, constrained WiiM transport, and
MediaServer advertisement remain on-link.

Consumers reach the collector through the gateway on the single declared API
port. The gateway routes these flows without source NAT, allowing the collector
firewall to restrict callers by their original addresses. Administrative SSH
remains limited to the trusted home LAN.

## Alternatives

- Keeping the collector on the home LAN would require protocol-specific relay
  and direct device flows across the VLAN boundary.
- Giving each consumer direct IoT access would remove the isolation the
  collector exists to provide.
- Adding interfaces on both LANs would turn the collector into another router
  and expand its firewall responsibilities.

## Consequences

- Device discovery and control remain local to the IoT broadcast domain.
- Consumers receive inventory and readings without direct IoT reachability.
- Collector DNS, default route, and deployment access must work from the IoT
  VLAN before a release can activate.
- Topology changes remain reviewed repository changes; routine tokens and
  credentials remain machine state.

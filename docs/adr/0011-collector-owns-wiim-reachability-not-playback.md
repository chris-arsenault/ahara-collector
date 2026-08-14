# 0011 — Collector owns WiiM reachability, not playback semantics

- Status: Accepted
- Date: 2026-08-14

## Context

Airwave cannot discover or control WiiM players after they move to the IoT
VLAN. Reopening renderer SSDP, HTTP, and HTTPS from the server subnet would
make the collector an inventory sidecar while leaving the original network
exposure intact. Moving Airwave's playback, grouping, queue, and response
parsing into the collector would duplicate application behavior in the wrong
service.

The WiiMs also discover Airwave's UPnP MediaServer through SSDP. Relaying that
traffic across VLANs works, but the collector can advertise the existing
server locally without interpreting its ContentDirectory protocol.

## Decision

The collector owns device reachability: on-link MediaRenderer discovery,
native inventory, a registry-constrained transport for the three advertised
UPnP control services and LinkPlay HTTPS, and local advertisement of a leased
Airwave MediaServer registration. Airwave owns all commands and response
semantics.

Airwave authenticates with its own bearer token. That token can use only the
`/wiim` routes and cannot read sensor streams or metrics; the House Sensors
token cannot use the WiiM routes. A control request names a device ID and a
fixed service route. Collector code resolves both the address and device-
advertised control path, disables redirects, and bounds response bodies, so
the API is not a general HTTP proxy.

MediaServer registrations must name the configured TrueNAS address, port,
and `/device.xml` path. They expire unless Airwave refreshes them. While a
lease is active, the collector answers on-link searches and sends the same
five SSDP advertisements Airwave previously emitted.

## Consequences

- Airwave can keep its playback and grouping implementation while losing all
  direct IoT reachability.
- The collector holds no music library, Airwave database state, or playback
  model.
- Compromise of either consumer token does not grant the other consumer's API
  surface.
- The temporary cross-VLAN SSDP relay remains only through migration and can
  be removed after Airwave uses the registration API.

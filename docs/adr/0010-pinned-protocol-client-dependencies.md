# 0010 — Use pinned HTTP/TLS and XML clients for device protocols

- Status: Accepted
- Date: 2026-08-14

## Context

ADR-0005 kept the collector service dependency-free so its appliance build
would remain offline and reproducible. That was practical while the service
spoke small HTTP/1.1 device protocols and test-vectored KLAP cryptography.

The WiiM adapter must fetch and parse UPnP descriptions and proxy the devices'
self-signed HTTPS API. Hand-written TLS would be unsafe. Delegating dynamic
device routing and registry authorization to nginx would split one security
decision across Nix configuration and Rust code, and it would be harder to
test than an ordinary service handler.

## Decision

The service uses pinned Rust HTTP/TLS and XML parsing libraries for device
protocols. `Cargo.lock` is committed and the pinned Nix flake supplies the
complete dependency closure, so builds remain offline and reproducible.

Nginx continues to terminate only the collector API's inbound TLS. Collector
code resolves registered device identities, validates routes, and performs
outbound HTTP or HTTPS. KLAP cryptography remains in the existing
test-vectored implementation rather than adding a general cryptography API.

## Consequences

- Device routing and authorization remain unit-testable Rust code.
- The crate dependency closure is larger than ADR-0005 allowed, but it is
  locked and built without runtime registry access.
- New dependencies still require a concrete protocol need and review; this
  decision is not permission to add an application framework.

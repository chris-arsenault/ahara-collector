# 0005 — The service is dependency-free, including its KLAP crypto

- Status: Accepted
- Date: 2026-08-05

## Context

The Kasa KP125M plugs only speak KLAP: SHA-1/SHA-256 seed authentication
and AES-128-CBC payloads. Everything else the service does (HTTP, JSON,
SSDP, spooling) is comfortably std-only, matching the ahara-vpn convention
that appliance services build offline from a pinned flake with no crate
registry. Crypto is the one place where "write it yourself" is normally the
wrong answer.

## Decision

Stay dependency-free and implement the three primitives in-crate, each
pinned to published vectors (FIPS 180-4, FIPS 197, SP 800-38A, RFC 4648)
in the unit tests. The threat model keeps this proportionate: KLAP protects
device-control credentials on the local LAN against passive observation; it
is not a TLS stack, carries no public exposure, and python-kasa's
implementation of the same construction is the interoperability reference.

The Kasa module is marked experimental until a real KP125M confirms the
handshake; the TrueNAS voltage collector remains the production path until
then (docs/integration.md, cutover).

## Alternatives considered

- **Pull in RustCrypto crates** — reintroduces registry access to appliance
  builds (fixed-output fetches work, but the offline-build property dies)
  for three well-vectored primitives.
- **Skip Kasa support** — leaves the TrueNAS collector reaching through the
  gateway to home-LAN devices forever, which ADR-0001 exists to end.

## Consequences

- The crypto surface is ~400 lines with exhaustive vectors; any change to
  it must keep the vector tests intact.
- If a device ever demands KLAP v1 (MD5-based) it will fail the handshake
  loudly; support would be a deliberate addition, not a fallback.

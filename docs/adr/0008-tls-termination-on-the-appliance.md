# 0008 — The pull API terminates TLS on the appliance

- Status: Accepted
- Date: 2026-08-06

## Context

The pull API carries a bearer token and the house's sensor readings, and
TrueNAS reaches it routed across two subnets through the VP2440 — a genuine
wire, not a loopback hop. The service itself is a dependency-free Rust binary
whose hand-written HTTP parsing is deliberately minimal (ADR-0005); giving it
a TLS stack would mean either a large dependency tree or hand-rolled
cryptography on the path that holds a private key. The gateway serves the
`collector.local.ahara.io` record, and ahara-vpn ADR-0015 establishes how an
appliance obtains a publicly-trusted certificate for a name under that
subtree: per-host Route53 DNS-01 with a credential scoped to its own
challenge record.

## Decision

nginx terminates TLS on the appliance's own address and proxies to the
collector service, which keeps speaking plain HTTP on its existing port. The
certificate for `collector.local.ahara.io` is issued and renewed here through
Route53 DNS-01 with a credential held as host state alongside the device
credentials and the API token. The plaintext leg is a connection from the
host to itself and never reaches a wire; the service's private key surface
stays zero.

## Alternatives considered

- **TLS inside the collector service** — one process, no proxy, but either a
  rustls/openssl dependency tree in a binary built offline from a pinned
  flake (ADR-0005) or hand-written TLS. Rejected on both counts.
- **A self-signed certificate on the terminator** — no cloud credential, but
  every consumer must either pin the certificate or disable verification, and
  the latter is what plaintext already gets you. The DNS record and the ACME
  path exist, so trust costs one credential.
- **Leave the API plaintext and rely on the inspected gateway flow** —
  Suricata sees the traffic precisely because anyone on the path can; a
  bearer token in the clear is the thing being fixed.
- **Terminate on the gateway instead** — moves the plaintext leg onto the
  wire between gateway and appliance, which is the problem restated.

## Consequences

- Consumers connect to `https://collector.local.ahara.io:8443` and verify a
  public chain; the plain port stays bound until the TrueNAS puller cuts over
  (docs/backlog.md), after which its firewall opening is removed.
- The appliance holds one long-lived AWS credential, scoped to changing one
  TXT record. Absent it, nginx serves a self-signed placeholder and every
  other function of the appliance is unaffected. That rests on gating the
  ordering unit (`acme-order-renew-<host>.service`) and never the unit that
  generates the placeholder — gating the latter leaves nginx with no
  certificate and the API unreachable.
- The deploy health gate checks that the terminator answers, so a release
  that cannot serve TLS rolls back rather than silently taking the API off
  the network.
- nginx is a new service on a host that previously ran only the collector and
  sshd, and it must survive binding an address networkd assigns late.
- Certificate expiry is exported as a textfile metric; a renewal that stops
  working is visible before a consumer's verification fails.

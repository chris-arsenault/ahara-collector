# 0008 — The pull API terminates TLS on the appliance

- Status: Accepted
- Date: 2026-08-06

## Context

The pull API carries a bearer token and the house's sensor readings, and
TrueNAS reaches it routed across two subnets through the VP2440 — a genuine
wire, not a loopback hop. The service itself is a Rust device-protocol binary
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
plaintext leg is a connection from the host to itself and never reaches a
wire; the service's private key surface stays zero.

The certificate comes from the machine-identity appliance, which obtains and
distributes it (ahara-vpn ADR-0015). This appliance runs no ACME client and
holds no cloud credential: one machine holds the DNS credential for the whole
household, because a credential per appliance grows the manual work with the
number of appliances.

There is no locally generated stand-in. nginx does not start without a
certificate, so an appliance that cannot obtain one fails its deploy instead
of serving a placeholder that would make the misconfiguration invisible.

## Alternatives considered

- **TLS inside the collector service** — one process, no proxy, but either a
  rustls/openssl dependency tree in a binary built offline from a pinned
  flake (ADR-0005) or hand-written TLS. Rejected on both counts.
- **A self-signed certificate on the terminator, permanently** — no cloud
  credential anywhere, but every consumer must either pin the certificate or
  disable verification, and the latter is what plaintext already gets you.
- **This appliance running its own ACME client** — renews independently with
  no distribution channel to build, at the cost of a long-lived cloud
  credential on every machine that terminates TLS.
- **Leave the API plaintext and rely on the inspected gateway flow** —
  Suricata sees the traffic precisely because anyone on the path can; a
  bearer token in the clear is the thing being fixed.
- **Terminate on the gateway instead** — moves the plaintext leg onto the
  wire between gateway and appliance, which is the problem restated.

## Consequences

- Consumers connect to `https://collector.local.ahara.io:8443`; the plain
  port stays bound until the TrueNAS puller cuts over (docs/backlog.md),
  after which its firewall opening is removed.
- The appliance holds no cloud credential, so compromising it yields none.
  Until the machine-identity appliance distributes a trusted certificate,
  consumers must pin or skip verification, which is the interim cost of not
  putting a credential here.
- The deploy health gate checks that the terminator answers, so a release
  that cannot serve TLS rolls back rather than silently taking the API off
  the network.
- nginx is a new service on a host that previously ran only the collector and
  sshd, and it must survive binding an address networkd assigns late.
- Certificate expiry is exported as a textfile metric; a renewal that stops
  working is visible before a consumer's verification fails.

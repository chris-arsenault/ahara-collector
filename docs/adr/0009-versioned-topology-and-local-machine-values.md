# 0009 — Version topology and keep machine values local

- Status: Accepted; narrows [ADR-0004](0004-pull-deployment-pattern-reused.md)
- Date: 2026-08-13

## Context

The former `site-values.json` stored the collector's static address and service
settings beside its interface MAC and administrator keys. This avoided Git for
every change, but it also kept rare topology changes out of review and made
unrelated values share one host-local file.

Only changes made during normal operation need to avoid Git. Moving the
collector to another VLAN or changing its static address is an infrequent
topology change that must stay synchronized with the gateway flows and
consumers.

## Decision

The collector composes two configuration stores:

- `hosts/collector/topology.json` is versioned. It owns network topology,
  deployment settings, listener ports, module settings, and spool limits.
- `/var/lib/ahara-collector/machine-values.json` stays on the appliance. It
  owns the interface MAC and administrator keys.

Device credentials, the API token, certificates, and service spool remain in
their existing dedicated host-state paths; none belongs in either
configuration store.

The updater overlays only `machine-values.json` onto a fetched release. The
first split release accepts the former combined `site-values.json` only to
read machine fields, while versioned topology wins. After activation, a
one-shot service extracts those machine fields atomically and archives the
legacy file.

## Consequences

- The move to `192.168.30.2` and later network or service topology changes go
  through Git, CI, and the normal release path.
- Replacing the NIC or administrator key changes only machine-local state and
  can trigger a rebuild without a commit.
- Bootstrap no longer accepts address, CIDR, router, DNS, or TrueNAS flags; it
  reads those values from the selected release.

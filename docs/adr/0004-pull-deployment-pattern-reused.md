# 0004 — Deployment reuses the ahara-vpn pull pattern

- Status: Accepted
- Date: 2026-08-05

## Context

The appliance needs unattended updates without inbound access from any
build machine. ahara-vpn's gateway already proved a shape for exactly this
class of host: CI advances a `release` ref; the host polls it, overlays its
host-state values, builds locally, and gates activation on a health check
with rollback (its ADR-0001, ADR-0006, ADR-0008).

## Decision

Adopt the pattern unchanged, renamed for this host: `s13-update` timer and
service, `(revision, values-hash)` change key so a values-only edit
redeploys, first-boot generated read-only deploy key for the private repo,
pinned GitHub host key, health-check gate with rollback to the previous
generation, and the store at `/var/lib/ahara-collector/site-values.json`
seeded by the bootstrap installer.

The values file is JSON (as ahara-vpn migrated to) but is edited on the
host directly — this appliance has no configuration API. If one proves
necessary it needs its own ADR; the gateway's audited-API rationale
(operators without shell access) does not hold here yet.

## Consequences

- Deploys need no credentials anywhere but the appliance's own deploy key.
- The updater self-invokes `switch-to-configuration`, so its unit carries
  the same self-upgrade guards (`restartIfChanged = false`,
  `X-StopOnRemoval = false`) the gateway documented.
- Placeholder values stay committed so CI and the VM test build the real
  configuration shape.

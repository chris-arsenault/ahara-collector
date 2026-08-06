# 0007 — One reading stream per module; consumers never share a batch

- Status: Accepted
- Date: 2026-08-06

## Context

With a single spool, a batch mixes every module's readings and one ack
deletes it, so exactly one consumer can drain the appliance. house-sensors
has one consumer per measurement (the environment collector and the volt
collector, separately credentialed and separately deployed), and the
appliance's charter adds more producers over time (radio modules). A
single stream forces a TrueNAS-side fan-out process that must learn every
new module before its readings can flow.

## Decision

The appliance consolidates readings per consumer: each module gets its own
bounded spool (a subdirectory under the spool root), `GET /readings/next`
takes a required `module` parameter, and acks name the module alongside
the batch id. `/ingest` routes each pushed envelope to its declared
module's spool, with the module name validated to stay filesystem- and
URL-safe. A new module is a new stream that exists as soon as something
produces into it; its consumer subscribes when ready, and no existing
consumer or fan-out changes.

## Alternatives considered

- **Single stream, one TrueNAS drain that fans out by module.** Couples
  every measurement's delivery to one process and one ack, and that
  process needs a mapping update for each new module before the module's
  data can land anywhere.
- **Single stream, per-consumer cursors on the appliance.** Batches
  survive until every subscribed consumer acks, which makes the appliance
  track consumer identity and turns one stalled consumer into everyone's
  disk-cap problem.

## Consequences

- Consumers are independent: a stalled or failed drain loses only its own
  module's oldest readings once that module's cap is hit.
- The disk budget multiplies by module count — the byte caps are per
  module, so the site config's `max_bytes` bounds each stream, not the
  total.
- A pushed module name nobody consumes accumulates readings in its own
  bounded spool and sheds oldest segments; nothing else degrades.
- The drain contract (docs/integration.md) carries the module explicitly
  in both directions.

# 0002 — TrueNAS pulls readings; the collector holds no upstream credentials

- Status: Accepted
- Date: 2026-08-05

## Context

Buffered readings must reach InfluxDB on TrueNAS through the gateway's
default-drop firewall. Either the collector pushes to TrueNAS (holding an
InfluxDB token and needing a home→servers flow toward the service host), or
TrueNAS pulls from the collector (holding the collector's API token and
needing one servers→home flow). Both cost exactly one declared flow.

## Decision

TrueNAS pulls. The collector serves its spool on the single API port,
authenticated by a bearer token generated on the appliance at first boot;
a TrueNAS-side job drains batches and writes them to InfluxDB, acknowledging
each batch after the write succeeds.

The direction is a containment decision: the collector faces the least
trusted devices in the house, so it holds device credentials only.
Compromising it yields no token that reaches into TrueNAS — the InfluxDB
admin token never leaves the server subnet, and the firewall never permits
the collector to originate connections to TrueNAS services.

## Alternatives considered

- **Push to InfluxDB directly** — one fewer moving part on TrueNAS, but the
  IoT-facing box gains a credential for (today) an admin-scoped database
  token and a standing allowed flow into the server subnet.
- **Push to a dedicated ingest endpoint with a narrow token** — still a
  standing collector→TrueNAS flow, and a second token lifecycle to manage.

## Consequences

- Delivery is at-least-once: a batch is deleted only on acknowledgement, so
  a crashed pull re-serves it and duplicate writes are possible. Influx
  writes are idempotent per (measurement, tags, timestamp), which makes
  duplicates harmless.
- The spool is bounded and drops oldest segments when full (telemetry, not
  a ledger): an extended TrueNAS outage costs the oldest readings, never
  the appliance's disk.
- The pull job and its flow declarations live with their owners:
  docs/integration.md specifies both.

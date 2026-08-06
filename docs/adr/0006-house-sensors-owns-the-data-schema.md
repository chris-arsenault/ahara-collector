# 0006 — house-sensors owns the data schema; the collector ships device-native readings

- Status: Accepted
- Date: 2026-08-06

## Context

The Influx schema — measurement names, field names, units, bucket
routing — was implemented twice: the collector built finished line
protocol (mirroring house-sensors' names, down to unit conversions like
Kasa milliwatts → `power` watts), and house-sensors' downstream consumers
(buckets, downsampler, dashboards) referenced the same names on TrueNAS.
Any schema change required coordinated updates across two repos on two
hosts.

The consumers cannot move: InfluxDB, the downsampler, and the dashboards
run on TrueNAS. The appliance's charter is the opposite direction — a
credential and communication hub for device protocols, eventually
including non-HTTP radio collection, where a received frame has no
inherent storage name at all.

## Decision

house-sensors is the only owner of the data schema. The collector emits
one JSON envelope per reading — `module`, `device` identity, collector
`timestampNs`, and `values` holding the device's payload verbatim (vendor
keys, vendor units, no renaming) — and the existing house-sensors
collectors change their input from direct device polling (which the
gateway firewall blocks) to draining the appliance's API, keeping their
schema mapping, bucket writes, and everything downstream unchanged. No new
component is added on either side.

The dividing line is protocol decode versus storage mapping: decoding a
vendor payload or radio frame into named values is inseparable from
speaking the protocol and lives in the collector module; deciding what any
value is called in Influx, in which unit, in which bucket, happens only in
house-sensors. The envelope vocabulary tracks device firmware, which
changes rarely and on its own schedule; schema churn is a single-repo
change on TrueNAS.

## Alternatives considered

- **Collector emits finished line protocol** (the initial
  implementation). Schema knowledge lands in both repos: the collector
  names fields, TrueNAS consumers reference them, and every rename is a
  lockstep deploy.
- **Collector writes InfluxDB directly and owns the schema.** Removes the
  TrueNAS-side translation but not the TrueNAS-side references — dashboards
  and the downsampler still name the same fields — so the schema stays in
  two places, and the IoT-facing appliance gains an Influx credential
  ADR-0002 deliberately keeps off it.
- **A new translation container on TrueNAS.** Same ownership outcome, but
  adds a component when the existing house-sensors collectors already
  contain the mapping code and only need their input source changed.

## Consequences

- Adding a device module is additive: the collector ships a new `module`
  name, house-sensors adds a mapping when ready; unknown modules are
  counted and dropped (or parked) on TrueNAS, never half-written. No
  lockstep deploy exists in either direction.
- The envelope format (docs/integration.md) is the cross-repo contract and
  the only shared vocabulary; it changes when device firmware changes, not
  when analytics naming does.
- house-sensors carries alias handling for firmware variants (bare versus
  suffixed sensor keys) and all unit conversion; the collector's only
  timestamp logic is measurement-time correction from device-reported
  sample age, which needs the poll-time clock only the appliance has.
- Readings in the spool are JSON envelopes, larger than line protocol for
  the same data; the spool remains bounded and oldest-dropped, so the cost
  is capacity, not correctness.

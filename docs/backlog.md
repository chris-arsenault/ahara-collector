# Backlog

Planned-but-not-built work. Each item is a positive assertion of
future-state behavior.

## Collector

- The Kasa module is validated against a real KP125M and loses its
  experimental marker; discovery, handshake, and field mappings are
  confirmed against python-kasa output side by side.
- The TrueNAS puller drains over `https://collector.local.ahara.io:8443`
  with chain verification, after which the plain port's firewall opening
  and the gateway's `truenas-to-collector-pull` flow are removed.
- Sensor firmware pushes readings to `POST /ingest` on its own schedule,
  and polling becomes the fallback rather than the only path.

## Cutover

- The TrueNAS environment and voltage collectors are retired; the
  collector is the sole producer for `environment-data` and
  `voltage-data`, and the gateway's TrueNAS→IoT flows are removed
  (docs/integration.md tracks the order).

## Observability

- Collector metrics are scraped into the platform observability stack and
  a dashboard ships from this repo (spool depth, poll failure rates, WiiM
  inventory and MediaServer discovery counters).

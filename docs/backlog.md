# Backlog

Planned-but-not-built work. Each item is a positive assertion of
future-state behavior.

## Collector

- The Kasa module is validated against a real KP125M and loses its
  experimental marker; discovery, handshake, and field mappings are
  confirmed against python-kasa output side by side.
- The pull API serves TLS (self-signed, the ahara-vpn config-API pattern),
  so the bearer token never crosses the gateway path in plaintext.
- Sensor firmware pushes readings to `POST /ingest` on its own schedule,
  and polling becomes the fallback rather than the only path.

## Cutover

- The TrueNAS environment and voltage collectors are retired; the
  collector is the sole producer for `environment-data` and
  `voltage-data`, and the gateway's TrueNAS→IoT flows are removed
  (docs/integration.md tracks the order).

## Network

- The appliance moves to the dedicated gateway-served IoT VLAN when
  ahara-vpn adds it — a re-address in the configuration store, plus new
  gateway flow sources.

## Observability

- Collector metrics are scraped into the platform observability stack and
  a dashboard ships from this repo (spool depth, poll failure rates, SSDP
  relay counters).

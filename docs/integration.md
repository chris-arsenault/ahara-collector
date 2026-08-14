# Integration: ahara-vpn flows, airwave, and the house-sensors drain

The appliance's external dependencies are declared elsewhere and owned by
their repos. This page is the contract for each. The examples use the current
site addresses; the owning repositories remain authoritative.

## ahara-vpn: gateway flows

Three active flows live in `hosts/gateway/site.nix` `allowedFlows`. All three
are ordinary forward flows and pass through Suricata inspection:

```nix
{
  # The readings pull (ahara-collector ADR-0002): the house-sensors
  # collectors drain the appliance's spool over its single API port.
  name = "truenas-to-collector-pull";
  description = "TrueNAS readings pull from the collector API";
  sourceZone = "servers";
  source = truenasIp;
  destZone = "iot";
  destination = collectorIp;
  inspect = true;
  protocol = "tcp";
  ports = [ 8850 ];
}
{
  # Airwave uses this now; the readings puller cuts over to the same endpoint.
  name = "truenas-to-collector-api-tls";
  description = "TrueNAS consumers reach the collector API over TLS";
  sourceZone = "servers";
  source = truenasIp;
  destZone = "iot";
  destination = collectorIp;
  inspect = true;
  protocol = "tcp";
  ports = [ 8443 ];
}
{
  # Players fetch Airwave's UPnP description and media streams directly.
  name = "iot-to-airwave-media";
  description = "WiiM devices fetch Airwave media descriptions and streams";
  sourceZone = "iot";
  source = iotLanCidr;
  destZone = "servers";
  destination = truenasIp;
  inspect = true;
  protocol = "tcp";
  ports = [ 7882 ];
}
```

These flows and the `collector.local.ahara.io` record are declared in the
gateway's `site.nix`; `collectorIp` is one of its site values. Once the
house-sensors collectors read from the collector API instead of polling
devices, the old TrueNAS-to-device discovery and polling flows are absent.
The plain-port flow retires with the puller's TLS cutover. There are no
cross-VLAN SSDP or TrueNAS-to-WiiM control flows.

## airwave: collector API

Airwave reaches WiiMs only through the collector TLS endpoint:

```
AIRWAVE_COLLECTOR_URL=https://collector.local.ahara.io:8443
AIRWAVE_COLLECTOR_TOKEN=<value from /var/lib/ahara-collector/airwave-token>
```

The suggested SSM path is `/ahara/airwave/collector/api-token`. This token is
distinct from `/ahara/house-sensors/collector/api-token`: it can list and
probe WiiMs, use their fixed transport routes, and renew the MediaServer
lease, but it cannot read sensor streams or metrics.

Airwave polls `GET /wiim/devices`. It sends UPnP requests through
`POST /wiim/<id>/upnp/{av-transport,rendering-control,play-queue}` and LinkPlay
commands through `GET /wiim/<id>/linkplay?<original query>`. When group state
names a renderer absent from inventory, Airwave may call `POST /wiim/probe`
with `{"ip":"192.168.30.x"}`; the collector validates the address and device
description before adding it.

Airwave renews `PUT /wiim/media-server` before the lease expires:

```json
{
  "uuid": "<Airwave UPnP UUID>",
  "location": "http://192.168.66.3:7882/device.xml",
  "server": "Linux/1.0 UPnP/1.0 Airwave/<version>",
  "leaseSeconds": 1200
}
```

The collector then emits Airwave's five existing MediaServer advertisements
on the IoT LAN and answers WiiM searches locally.

## house-sensors: drain the collector instead of polling devices

The existing house-sensors collectors keep everything downstream of
input — measurement and field naming, unit conversion, bucket writes,
downsampler, dashboards — and change only where readings come from: the
appliance's API replaces direct device polling, which the gateway firewall
blocks from the server subnet. No new container is introduced.

house-sensors is the sole owner of the data schema (ADR-0006). The
appliance ships device-native readings and never emits a measurement,
field, or bucket name.

Each module is its own stream (ADR-0007): the environment collector
drains `module=envSensors`, the volt collector drains `module=kasa`, and
the two never see each other's batches. A future module is a new stream
with a new consumer, not a change to any existing one.

### Reading envelopes

`GET /readings/next` serves a batch of newline-separated JSON envelopes,
one per reading:

```json
{
  "module": "envSensors",
  "device": {
    "ip": "192.168.30.42",
    "name": "ATOM3U-ENV3-005",
    "model": "ENV3",
    "deviceId": "ATOM3U-ENV3-005",
    "tags": { "room": "office lab" }
  },
  "timestampNs": 1782788400000000000,
  "values": {
    "temperature_c": 21.5,
    "humidity": 45.1,
    "pressure_pa": 101325.0,
    "sample_age_ms": 50.0
  }
}
```

- `module` — which collector module produced the reading: `envSensors`
  (AtomS3U environment sensors) or `kasa` (KP125M plugs). The module also
  names the stream the reading is served on.
- `device` — identity as discovered: `ip` always; `name`, `model`,
  `deviceId`, and user `tags` when the device reports them.
- `timestampNs` — measurement time in Unix nanoseconds, computed on the
  appliance (for env sensors, corrected by the device-reported sample age
  when present).
- `values` — the device's own payload, verbatim. Env sensors: the
  `/sensors` JSON as the firmware sent it. Kasa: the `get_energy_usage`
  result with vendor keys and units (`current_power` in mW, `today_energy`
  in Wh, `voltage_mv`, `current_ma`).

house-sensors maps envelopes to its schema: `envSensors` readings become
the `environment` measurement in `environment-data`, `kasa` readings
become `voltage_monitoring` in `voltage-data`, with all field naming and
unit conversion applied there.

### Drain cycle (per consumer)

1. `GET https://collector.local.ahara.io:8443/readings/next?module=<name>`
   with `authorization: Bearer <token>`, verifying the chain normally — the
   appliance serves a publicly-trusted certificate (ADR-0008). The plain
   `http://192.168.30.2:8850` endpoint stays available until this cutover
   completes (docs/backlog.md).
   - `204` — stream empty; sleep (10 s is fine) and retry.
   - `200` — body `{"batchId": "...", "module": "...", "lines": "..."}`;
     `lines` is the newline-separated envelopes, all from this module.
2. Map each envelope and write to InfluxDB at
   `http://192.168.66.3:18086/api/v2/write?org=ahara&bucket=<bucket>&precision=ns`.
3. Only after every write succeeds:
   `POST /readings/ack` with `{"module": "...", "batchId": "..."}`.
4. On any failure, do not ack — the batch is re-served. Duplicate writes
   are safe (idempotent per measurement/tags/timestamp).

Configuration: `COLLECTOR_URL`
(`https://collector.local.ahara.io:8443`, resolved by the gateway's DNS),
`COLLECTOR_TOKEN` (suggested SSM path
`/ahara/house-sensors/collector/api-token`, populated from
`sudo cat /var/lib/ahara-collector/api-token`), plus the stack's existing
Influx settings. Poll every 10 s; each batch is at most one spool segment
(4 MiB by default).

## Cutover order

The gateway firewall blocks the house-sensors collectors from reaching
IoT-LAN devices directly, so switching their input to the appliance's API
is what restores data flow into the existing buckets, downsampler, and
dashboards.

1. Deploy the collector and verify `collector-health-check` and the VM-tested
   paths against real devices.
2. Store the Airwave-specific token, switch Airwave to the collector API,
   confirm inventory, control, grouping, and media browsing, then deploy the
   gateway policy without cross-VLAN SSDP or direct renderer-control flows.
3. Switch the house-sensors collectors to the drain input with the
   collector token; confirm `environment` and `voltage_monitoring` land in
   their buckets and the dashboards fill in again.
4. Validate the Kasa module against a real KP125M (ADR-0005).
5. Remove the house-sensors device-polling paths and the TrueNAS→IoT
   gateway flows they used.

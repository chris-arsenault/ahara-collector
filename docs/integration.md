# Integration: ahara-vpn flows and the TrueNAS pull job

The appliance's two external dependencies are declared elsewhere and owned
by their repos. This page is the contract for both. Addresses below use the
placeholder values; substitute the real site values.

## ahara-vpn: gateway flows

Three flows in `hosts/vp2440/site.nix` `allowedFlows`, replacing the
directed-broadcast pair `airwave-ssdp-discovery` / `airwave-ssdp-replies`
once the collector is live. All three are ordinary forward flows, so they
regain the Suricata inspection the gateway-hosted relay lost:

```nix
{
  # Airwave's M-SEARCH (source port 1901) and MediaServer NOTIFYs to the
  # collector appliance, which re-originates them on-link (ahara-collector
  # ADR-0001).
  name = "airwave-to-collector-ssdp";
  description = "Airwave SSDP discovery and announcements to the collector appliance";
  sourceZone = "servers";
  source = truenasIp;
  destZone = "home";
  destination = collectorIp; # 192.168.65.3
  inspect = true;
  protocol = "udp";
  ports = [ 1900 ];
}
{
  # Renderer replies to Airwave's fixed response port 1901, and relayed
  # WiiM MediaServer searches to Airwave's SSDP port 1900. Both originate
  # from the collector's fixed relay port.
  name = "collector-to-airwave-ssdp";
  description = "Collector SSDP replies and relayed searches to Airwave";
  sourceZone = "home";
  source = collectorIp;
  destZone = "servers";
  destination = truenasIp;
  inspect = true;
  protocol = "udp";
  ports = [
    1900
    1901
  ];
}
{
  # The readings pull (ahara-collector ADR-0002): TrueNAS drains the
  # collector's spool over its single API port.
  name = "truenas-to-collector-pull";
  description = "TrueNAS readings pull from the collector API";
  sourceZone = "servers";
  source = truenasIp;
  destZone = "home";
  destination = collectorIp;
  inspect = true;
  protocol = "tcp";
  ports = [ 8850 ];
}
```

`collectorIp` should come from the gateway's site values (a new
`collectorIp` value, or an `extraDnsHosts` entry — a `collector` DNS record
under `local.ahara.io` is worth adding at the same time). Once the TrueNAS
house-sensors containers retire, `truenas-to-iot-discovery`,
`iot-discovery-replies`, and `truenas-to-iot-poll` retire with them.

## airwave: target change

`AIRWAVE_SSDP_TARGETS` currently ends with the failed attempt's gateway
address (`192.168.66.1`). Point it at the collector instead:

```
AIRWAVE_SSDP_TARGETS=239.255.255.250,192.168.65.3
```

Nothing else in airwave changes: it still sends M-SEARCH from :1901,
NOTIFYs from :1900, and receives replies on :1901.

## TrueNAS: the pull job

A small container in the house-sensors stack drains the collector and
writes to InfluxDB. Contract:

1. `GET http://192.168.65.3:8850/readings/next` with
   `authorization: Bearer <token>`.
   - `204` — spool empty; sleep (10 s is fine) and retry.
   - `200` — body `{"batchId": "...", "lines": "..."}`; `lines` is
     newline-separated InfluxDB line protocol.
2. Route each line by measurement and POST to
   `http://192.168.66.3:18086/api/v2/write?org=ahara&bucket=<bucket>&precision=ns`:
   `environment` → `environment-data`, `voltage_monitoring` →
   `voltage-data` (anything else → drop and count).
3. Only after every write succeeds:
   `POST /readings/ack` with `{"batchId": "..."}`.
4. On any failure, do not ack — the batch is re-served. Duplicate writes
   are safe (idempotent per measurement/tags/timestamp).

Configuration: `COLLECTOR_URL`, `COLLECTOR_TOKEN` (suggested SSM path
`/ahara/house-sensors/collector/api-token`, populated from
`sudo cat /var/lib/ahara-collector/api-token`), plus the stack's existing
Influx settings. Poll every 10 s; each batch is at most one spool segment
(4 MiB by default).

## Cutover order

1. Deploy the collector; add the three gateway flows; verify
   `s13-health-check` and the VM-tested paths against real devices.
2. Switch `AIRWAVE_SSDP_TARGETS`; confirm WiiM discovery in airwave; remove
   the directed-broadcast flows from ahara-vpn.
3. Add the pull job with the collector token; watch both writers land in
   Influx (the TrueNAS collectors remain the production writers).
4. Validate the Kasa module against a real KP125M (ADR-0005) and compare
   readings.
5. Disable the TrueNAS environment/voltage collectors, then retire the
   TrueNAS→IoT gateway flows.

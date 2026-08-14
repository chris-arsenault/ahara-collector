# ahara-collector service

One Rust binary with a locked dependency closure. Run shape:

```
ahara-collector run --config <config.json> --token-file <path> \
  --airwave-token-file <path> --credentials <path>
```

The config document is rendered by Nix from `hosts/collector/site.nix`; the
scoped House Sensors and Airwave tokens and the credentials file are host state delivered through systemd
credentials. Modules: the Airwave SSDP relay (`ssdp.rs`, pure packet
processors over thin socket loops), the environment-sensor and Kasa
pollers (`sensors.rs`, `kasa.rs` — Kasa is experimental until validated on
hardware, ADR-0005), the reading-envelope format (`envelope.rs`, the
device-native contract of ADR-0006), the bounded spool (`spool.rs`), and
the single-port API (`api.rs`), and the registry-constrained WiiM transport
(`wiim.rs`). `crypto.rs` carries the test-vectored
primitives KLAP needs.

`cargo test` runs the unit suite; the VM test exercises the composed
binary against mock devices.

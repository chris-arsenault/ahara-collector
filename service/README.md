# ahara-collector service

One dependency-free Rust binary. Run shape:

```
ahara-collector run --config <config.json> --token-file <path> --credentials <path>
```

The config document is rendered by Nix from `hosts/s13/site.nix`; the
token and credentials files are host state delivered through systemd
credentials. Modules: the Airwave SSDP relay (`ssdp.rs`, pure packet
processors over thin socket loops), the environment-sensor and Kasa
pollers (`sensors.rs`, `kasa.rs` — Kasa is experimental until validated on
hardware, ADR-0005), the bounded spool (`spool.rs`), and the single-port
API (`api.rs`). `crypto.rs` carries the test-vectored primitives KLAP
needs.

`cargo test` runs the unit suite; the VM test exercises the composed
binary against mock devices.

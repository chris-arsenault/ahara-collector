# tests

Two layers, mirroring ahara-vpn's rationale:

- `site-validation.nix` — eval-time contract tests: the committed
  placeholder site must validate, and each deliberately broken variant
  must be rejected.
- `collector-vm.nix` — two-VM liveness test: the collector node runs the real
  host modules; a peer node plays the router, TrueNAS/Airwave, a WiiM
  renderer, and an environment sensor. Asserts what only a running system
  can prove: the MAC rename, firewall pins, the credentials-restart
  contract, WiiM inventory and transport, local MediaServer discovery,
  discovery → poll → spool → pull → ack, and the health gate.

Rust behavior (local SSDP response filtering, transport destination pins,
spool bounds, KLAP round-trips, line shapes) is unit-tested in the crate
itself: `cd service && cargo test`.

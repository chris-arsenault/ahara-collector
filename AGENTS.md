# Agent guide

Read this before editing. [README.md](README.md) has the repository map.

## Critical rules

- **Never push without being asked.** Pushing `main` deploys: CI advances
  `release` and the appliance activates it within minutes.
- **`hosts/collector/site.nix` is the sole source of addresses and ports.** No
  literal IP, CIDR, MAC, or port belongs in any module, script, or the Rust
  service — the service receives topology as a rendered JSON config.
- **Secrets never enter the repo or the Nix store.** Device credentials and
  the API token are host state under `/var/lib/ahara-collector/`, passed to
  the service via systemd credentials. Public keys are the only key material
  that crosses the wire at bootstrap.
- **The Rust service stays dependency-free.** The appliance builds offline
  from a pinned flake; everything is std plus the test-vectored primitives
  in `service/src/crypto.rs`.
- **TLS terminates in nginx, never in the service** (ADR-0008). The
  certificate is publicly trusted and comes from the machine-identity
  appliance; this appliance runs no ACME client and holds no cloud
  credential. There is no placeholder to fall back to: without a certificate
  nginx does not start, which is what keeps a broken issuance visible.
- **The collector never speaks the data schema.** house-sensors owns every
  measurement, field, and bucket name (ADR-0006); this service ships
  device-native reading envelopes with verbatim device keys and units. The
  envelope format in `docs/integration.md` is the cross-repo contract.
- **`make ci` is canonical.** Run it before considering any change done.

## Code map

| Area | Where | Notes |
| ---- | ----- | ----- |
| Site values contract | `lib/site-assertions.nix` | Validates the derived site at eval time |
| Host modules | `hosts/collector/*.nix` | One concern per file; composition in `configuration.nix` |
| SSDP relay | `service/src/ssdp.rs` | Pure packet processors + thin socket loops |
| Sensor pollers | `service/src/sensors.rs`, `service/src/kasa.rs` | Kasa KLAP is experimental until validated on hardware |
| Spool | `service/src/spool.rs` | Bounded, oldest-dropped, ack-deletes |
| API | `service/src/api.rs` | Single port; bearer for pulls, Basic for device pushes |
| TLS terminator | `hosts/collector/tls.nix` | nginx in front of the API port |
| Updater + health gate | `hosts/collector/deployment.nix` | The ahara-vpn ADR-0001/0008 pattern |
| Installer | `scripts/bootstrap-collector.sh` | scp a public key, run one command |

## Conventions

- Docs follow the ahara-vpn style: index-style README, ADRs in
  `docs/adr/NNNN-*.md` for decisions with trade-offs, future work only in
  `docs/backlog.md`, user-visible changes in `CHANGELOG.md`.
- Tests are layered: eval-time contract tests in `tests/site-validation.nix`,
  Rust unit tests in-crate, whole-system behavior in `tests/collector-vm.nix`.

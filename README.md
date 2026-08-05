# ahara-collector

The Ahara home-LAN IoT collector appliance: a NixOS host (Beelink Mini S13)
that lives on the home LAN, relays SSDP between Airwave and the WiiM
players, polls the house's IoT sensors with locally held credentials,
buffers readings in a bounded on-disk spool, and serves everything to
TrueNAS through one authenticated API port.

It replaces the gateway-hosted SSDP relay attempt in ahara-vpn: WiiM
devices ignore SSDP sourced from the routed server subnet, so discovery has
to originate on the WiiM subnet itself (ADR-0001).

## Repository map

| Path | Contents |
| ---- | -------- |
| [`hosts/s13/`](hosts/s13/) | The appliance's NixOS configuration; `site.nix` is the single source of truth |
| [`service/`](service/) | The collector service: dependency-free Rust, one binary |
| [`lib/`](lib/) | Pure-nix site validation |
| [`scripts/`](scripts/) | `bootstrap-s13` installer and the site-values renderer |
| [`tests/`](tests/) | Eval-time site validation and the two-VM liveness test |
| [`docs/`](docs/) | [Architecture](docs/architecture.md), [runbook](docs/runbook.md), [integration](docs/integration.md), [ADRs](docs/adr/), [backlog](docs/backlog.md) |

## Commands

| Command | Purpose |
| ------- | ------- |
| `make ci` | Format check, flake validation, unit tests |
| `make test-vm` | Build and run the VM test |
| `cargo test` (in `service/`) | Rust unit tests |
| `nix run .#bootstrap-s13` | Install the appliance (run on the NixOS installer; see runbook) |

## Deployment

CI validates `main` and advances the `release` branch; the appliance polls
that ref every two minutes, overlays its host values, builds, activates,
and keeps the release only when the health check passes. First install is
one `bootstrap-s13` command ([runbook](docs/runbook.md)).

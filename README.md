# ahara-collector

The Ahara IoT collector appliance: a NixOS host (Beelink Mini S13)
that lives on the dedicated IoT LAN, maintains a validated WiiM inventory and
registry-constrained Airwave transport, advertises Airwave's MediaServer
locally, and polls the house's IoT sensors with locally held credentials,
buffers readings in a bounded on-disk spool, and serves everything to
TrueNAS through one authenticated API port, served over TLS at
`collector.local.ahara.io`.

WiiM devices ignore SSDP sourced from the routed server subnet. The collector
therefore owns their on-link discovery and reachability while Airwave retains
playback, grouping, and protocol semantics (ADR-0011).

## Repository map

| Path | Contents |
| ---- | -------- |
| [`hosts/collector/`](hosts/collector/) | The appliance's NixOS configuration; `site.nix` is the single source of truth |
| [`service/`](service/) | The collector service: pinned Rust device-protocol clients, one binary |
| [`lib/`](lib/) | Pure-nix site validation |
| [`scripts/`](scripts/) | `bootstrap-collector` installer and the machine-values renderer |
| [`tests/`](tests/) | Eval-time site validation and the two-VM liveness test |
| [`docs/`](docs/) | [Architecture](docs/architecture.md), [runbook](docs/runbook.md), [integration](docs/integration.md), [ADRs](docs/adr/), [backlog](docs/backlog.md) |

## Commands

| Command | Purpose |
| ------- | ------- |
| `make ci` | Format check, flake validation, unit tests |
| `make test-vm` | Build and run the VM test |
| `cargo test` (in `service/`) | Rust unit tests |
| `nix run .#bootstrap-collector` | Install the appliance (run on the NixOS installer; see runbook) |

## Device credentials

The devices' credentials already live in SSM under the house-sensors
paths; render them into the credentials file rather than retyping them
(run from any AWS-credentialed machine):

```bash
get() { aws ssm get-parameter --with-decryption --query Parameter.Value --output text --name "$1"; }
cat > credentials.json <<EOF
{
  "envSensors": {
    "username": "$(get /ahara/house-sensors/environment-sensors/device-user)",
    "password": "$(get /ahara/house-sensors/environment-sensors/device-pass)"
  },
  "kasa": {
    "username": "$(get /ahara/house-sensors/volt/kasa-username)",
    "password": "$(get /ahara/house-sensors/volt/kasa-password)"
  }
}
EOF
```

Hand the file to `bootstrap-collector --credentials-file` at install, or upload
it later per the [runbook](docs/runbook.md) (scp, `install -m 0600` to
`/var/lib/ahara-collector/credentials.json`, restart `ahara-collector`).
Delete the local copy afterwards; it never belongs in a repo.

## Deployment

CI validates `main` and advances the `release` branch; the appliance polls
that ref every two minutes, overlays its machine values, builds, activates,
and keeps the release only when the health check passes. First install is
one `bootstrap-collector` command ([runbook](docs/runbook.md)).

# scripts

- `bootstrap-collector.sh` (+ `.nix` wrapper, exposed as `nix run .#bootstrap-collector`)
  — one-command install on the NixOS installer: discovers the NIC, renders
  and validates this machine's hardware/access identity with versioned
  topology, erases the disk with the checked-in disko layout, seeds the
  machine store (and optionally the
  credentials file), and installs. See the runbook for the full procedure.
- `render-machine-values.sh` — env-vars → `machine-values.json` renderer used by
  the bootstrap; plain bash, no nix.

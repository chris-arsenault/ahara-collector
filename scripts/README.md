# scripts

- `bootstrap-s13.sh` (+ `.nix` wrapper, exposed as `nix run .#bootstrap-s13`)
  — one-command install on the NixOS installer: discovers the NIC, renders
  and validates this machine's values, erases the disk with the checked-in
  disko layout, seeds the configuration store (and optionally the
  credentials file), and installs. See the runbook for the full procedure.
- `render-site-values.sh` — env-vars → `site-values.json` renderer used by
  the bootstrap; plain bash, no nix.

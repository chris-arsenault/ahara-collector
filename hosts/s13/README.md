# hosts/s13

The appliance's NixOS configuration. `configuration.nix` is a pure
composition root; each module owns one concern and reads every value from
`site.nix` (via `specialArgs`). `site-values.json` holds the committed
placeholders — a real machine's values are host state at
`/var/lib/ahara-collector/site-values.json`, overlaid by the updater on
every build.

| File | Concern |
| ---- | ------- |
| `site.nix` | Single source of truth, derived from the values store |
| `network.nix` | MAC→`lan0` rename, static addressing, input firewall |
| `collector.nix` | The collector service, its config rendering, token + credentials state |
| `tls.nix` | nginx TLS terminator for the API and its ACME certificate |
| `deployment.nix` | Pull updater and health-check gate |
| `hardening.nix` | Keyed SSH, no passwords, console autologin |
| `hardware-configuration.nix` | Beelink S13 boot/platform |
| `disko.nix` | ESP + ext4 root; disk path is a bootstrap flag |

# hosts/collector

The appliance's NixOS configuration. `configuration.nix` is a pure
composition root; each module owns one concern and reads the composed result
from `site.nix` (via `specialArgs`). Versioned `topology.json` holds network
and service settings. A real host's `machine-values.json` holds its interface
MAC and administrator keys and is overlaid by the updater on every build.

| File | Concern |
| ---- | ------- |
| `site.nix` | Composition of versioned topology and local machine values |
| `network.nix` | MAC→`lan0` rename, static addressing, input firewall |
| `collector.nix` | The collector service, its config rendering, token + credentials state |
| `tls.nix` | nginx TLS terminator for the API and its certificate |
| `deployment.nix` | Pull updater and health-check gate |
| `hardening.nix` | Keyed SSH, no passwords, console autologin |
| `hardware-configuration.nix` | Beelink S13 boot/platform |
| `disko.nix` | ESP + ext4 root; disk path is a bootstrap flag |

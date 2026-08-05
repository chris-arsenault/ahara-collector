# Runbook

## First install

Hardware: Beelink Mini S13 (Intel N150, 12 GB, NVMe), UEFI boot, its 2.5G
port cabled to the home LAN.

Decide before starting:

- The appliance's static address, outside the router's DHCP pool
  (`192.168.65.3` in the examples).
- Where the device credentials will come from (a `credentials.json` per
  ADR-0003, or upload later).

Your machine needs only `ssh` and `scp` — no nix, no checkout. Boot the
NixOS installer USB on the S13, set a temporary root password
(`sudo passwd`), note its DHCP address, then from your machine:

```bash
scp ~/.ssh/s13-ops.pub root@INSTALLER_IP:/tmp/ops.pub
scp credentials.json root@INSTALLER_IP:/tmp/credentials.json   # optional
ssh root@INSTALLER_IP
```

On the installer (the repo is private, so forward an agent with `ssh -A`
or use a token-backed `nix run` ref):

```bash
nix --extra-experimental-features 'nix-command flakes' \
  run 'git+ssh://git@github.com/chris-arsenault/ahara-collector#bootstrap-s13' -- \
  --disk /dev/disk/by-id/nvme-... \
  --key-file /tmp/ops.pub \
  --address 192.168.65.3 \
  --home-lan-cidr 192.168.65.0/24 \
  --router-ip 192.168.65.1 \
  --credentials-file /tmp/credentials.json
```

`--dry-run` first shows the rendered values and the disko plan without
touching the disk. The command discovers the NIC, renders and validates
this machine's values, erases the disk, seeds
`/var/lib/ahara-collector/site-values.json` (and the credentials file if
given), and installs. Nothing is committed to git.

## After first boot

1. SSH in: `ssh ops@192.168.65.3`.
2. Register the deploy key (read-only) on the GitHub repo so the updater
   can pull releases:
   `sudo cat /var/lib/ahara-collector/deploy-key.pub`
3. Read the API token and give it to the TrueNAS pull job
   ([integration.md](integration.md)):
   `sudo cat /var/lib/ahara-collector/api-token`
4. If credentials were not seeded at install:

   ```bash
   scp credentials.json ops@192.168.65.3:/tmp/credentials.json
   ssh ops@192.168.65.3 'sudo install -m 0600 -o root -g root \
     /tmp/credentials.json /var/lib/ahara-collector/credentials.json && \
     rm /tmp/credentials.json && sudo systemctl restart ahara-collector'
   ```

5. Verify: `s13-health-check` prints `health: all checks ok`, and
   `curl -s http://192.168.65.3:8850/health` answers from the home LAN.

## Routine operations

| Task | How |
| ---- | --- |
| Deploy a change | Merge to `main`; CI advances `release`; the appliance activates it within ~2 minutes |
| Force an update poll | `sudo systemctl start s13-update.service` |
| Change a host value | Edit `/var/lib/ahara-collector/site-values.json`; the next poll rebuilds (validation rejects typos before activation) |
| Rotate device credentials | Re-upload the file, `sudo systemctl restart ahara-collector` |
| See what the collector is doing | `journalctl -u ahara-collector -f` (structured `event=` lines) |
| Check the spool | `curl -s -H "authorization: Bearer $TOKEN" http://192.168.65.3:8850/metrics \| grep spool` |
| List discovered devices | `curl -s -H "authorization: Bearer $TOKEN" http://192.168.65.3:8850/devices` |

## Recovery

The appliance is rebuildable from the repo plus three pieces of host
state: `site-values.json`, `credentials.json`, and the two generated
identities (deploy key, API token — both regenerate on first boot; only
their registrations need redoing). Re-run the bootstrap, re-register the
new deploy key, hand the new API token to the pull job.

A bad release rolls itself back (health gate). A bad host-values edit
fails validation and never activates; fix the file and re-run
`s13-update`. If the machine is unreachable, the physical console
auto-logs-in as `ops`.

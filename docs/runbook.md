# Runbook

## First install

Hardware: Beelink Mini S13 (Intel N150, 12 GB, NVMe), UEFI boot, its 2.5G
port cabled to the IoT LAN.

Decide before starting:

- The appliance's static address, outside the router's DHCP pool
  (`192.168.30.2` in the examples).
- Where the device credentials will come from (a `credentials.json` per
  ADR-0003, or upload later).

Your machine needs only `ssh`, `scp`, and (for the credentials render) the
AWS CLI — no nix, no checkout.

One-time prep, from your machine.

Generate the admin keypair the `ops` user will trust (skip if
`~/.ssh/collector-ops` already exists):

```bash
ssh-keygen -t ed25519 -N "" -f ~/.ssh/collector-ops -C "collector-ops"
```

The public half is authorized on the appliance at install; the private key
stays on your machine (`ssh -i ~/.ssh/collector-ops ops@…`, or add it to
`~/.ssh/config` for the host).

Render the device credentials from the SSM parameters they already live
in:

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

(In the managed environment run the `aws` calls through `with-cred --`;
elsewhere any AWS-credentialed shell works. Delete the local file once it
is on the appliance.)

Boot the NixOS installer USB on the S13 — if no wire is available at the
bench, `nmtui` gets the installer online — then at its console:

```bash
sudo passwd                           # temporary ROOT password (installer-only)
test -d /sys/firmware/efi/efivars     # must succeed: UEFI boot
ip -brief address                     # note the address = INSTALLER_IP
ls -l /dev/disk/by-id/                # note the whole-disk id = INSTALL_DISK
```

The password exists only in the live installer and dies with it. Pick the
`nvme-…` entry that names the disk itself, not an `…-part1`-suffixed
partition (the bootstrap rejects partitions and unstable `/dev/nvme0n1`
paths). Then from your machine:

```bash
scp ~/.ssh/collector-ops.pub root@INSTALLER_IP:/tmp/ops.pub
scp credentials.json root@INSTALLER_IP:/tmp/credentials.json
ssh root@INSTALLER_IP
```

On the installer (the repo is public, so no credentials are involved):

```bash
nix --extra-experimental-features 'nix-command flakes' \
  run 'github:chris-arsenault/ahara-collector#bootstrap-collector' -- \
  --disk /dev/disk/by-id/<INSTALL_DISK> \
  --key-file /tmp/ops.pub \
  --credentials-file /tmp/credentials.json
```

`--dry-run` first shows the rendered machine values and the disko plan without
touching the disk. Topology comes from the selected release. The command
discovers the NIC, renders its interface identity and administrator keys,
validates them with that topology, erases the disk, seeds
`/var/lib/ahara-collector/machine-values.json` (and the credentials file if
given), and installs. Nothing is committed to Git.

## After first boot

1. SSH in: `ssh ops@192.168.30.2`.
2. Read the House Sensors token and give it to the house-sensors drain
   ([integration.md](integration.md)):
   `sudo cat /var/lib/ahara-collector/api-token`
3. Read the separately scoped Airwave token and store it at the Airwave
   stack's configured secret path:
   `sudo cat /var/lib/ahara-collector/airwave-token`
4. If credentials were not seeded at install:

   ```bash
   scp credentials.json ops@192.168.30.2:/tmp/credentials.json
   ssh ops@192.168.30.2 'sudo install -m 0600 -o root -g root \
     /tmp/credentials.json /var/lib/ahara-collector/credentials.json && \
     rm /tmp/credentials.json && sudo systemctl restart ahara-collector'
   ```

5. Verify: `collector-health-check` prints `health: all checks ok`, and
   `curl -s https://collector.local.ahara.io:8443/health` answers from the
   IoT LAN. No `-k`: the certificate is publicly trusted. If nothing answers,
   check `journalctl -u ahara-enroll.service` and
   `journalctl -u ahara-certificate.service`. Both services retry from systemd
   timers; starting them manually is only an acceleration or recovery step.

## Routine operations

| Task | How |
| ---- | --- |
| Deploy a change | Merge to `main`; CI advances `release`; the appliance activates it within ~2 minutes |
| Force an update poll | `sudo systemctl start collector-update.service` |
| Change topology or a service setting | Edit `hosts/collector/topology.json`; CI deploys the reviewed release |
| Replace the NIC or administrator key | Edit `/var/lib/ahara-collector/machine-values.json`, then start `collector-update.service` |
| Rotate device credentials | Re-upload the file, `sudo systemctl restart ahara-collector` |
| See what the collector is doing | `journalctl -u ahara-collector -f` (structured `event=` lines) |
| Check the spool | `curl -s -H "authorization: Bearer $TOKEN" https://collector.local.ahara.io:8443/metrics \| grep spool` |
| List sensor devices | `curl -s -H "authorization: Bearer $SENSOR_TOKEN" https://collector.local.ahara.io:8443/devices` |
| List WiiM devices | `curl -s -H "authorization: Bearer $AIRWAVE_TOKEN" https://collector.local.ahara.io:8443/wiim/devices` |
| Check certificate expiry | `cat /var/lib/ahara-collector/metrics/tls_cert.prom` |

## Recovery

The appliance is rebuildable from the repo plus `machine-values.json` and
`credentials.json`. Both API tokens regenerate on first boot; the repo is
public, so there is no repository credential to restore. Re-run the bootstrap
and hand each new token to its consumer.

A bad release rolls itself back (health gate). A bad machine-values edit
fails validation and never activates; fix the file and re-run
`collector-update`. If the machine is unreachable, the physical console
auto-logs-in as `ops`.

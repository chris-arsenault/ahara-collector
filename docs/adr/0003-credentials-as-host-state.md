# 0003 — Device credentials are one host-state file

- Status: Accepted
- Date: 2026-08-05

## Context

The environment sensors share one HTTP Basic user/password; the Kasa plugs
need the TP-Link account credentials. ahara-vpn's ADR-0013 deferred moving
sensor polling off TrueNAS until a "credential-file bootstrap contract"
existed — this is that contract. The repo convention (ahara-vpn ADR-0002)
is that no secret enters git, the Nix store, or a shell argument, and that
sops/agenix are not worth a new toolchain for this scale.

## Decision

All device credentials live in one root-owned 0600 JSON file,
`/var/lib/ahara-collector/credentials.json`:

```json
{
  "envSensors": { "username": "...", "password": "..." },
  "kasa": { "username": "...", "password": "..." }
}
```

It arrives either as a `--credentials-file` flag at bootstrap or by scp
afterwards (procedure in the runbook). systemd's `LoadCredential` hands it
to the sandboxed service; the service never reads the path directly, and
the unit's environment and command line stay secret-free. A missing or
empty file is not an error: modules without credentials idle and say so in
the journal, so the appliance deploys before its secrets and wakes when
they arrive.

## Alternatives considered

- **Prompting during bootstrap** — interactive installs conflict with the
  one-command bootstrap, and the value would transit the installer's shell
  history or environment.
- **agenix/sops-nix** — rejected for the same reason as ahara-vpn ADR-0002:
  the decryption key becomes a bootstrap secret with the same distribution
  problem.

## Consequences

- Rotation is an scp plus `systemctl restart ahara-collector`.
- The file is the one thing (besides the store and keys) to restore when
  rebuilding the appliance; the runbook's recovery section lists it.

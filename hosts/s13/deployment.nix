# Pull-based self-deployment, the ahara-vpn ADR-0001/ADR-0008 pattern. A
# timer polls the release branch; on a new revision (or changed host values)
# the appliance builds and activates it, then commits it as the boot default
# only when the health check passes. Activation or health failure rolls back
# to the previous generation. The health-check binary is also on PATH for
# manual runs.
{
  site,
  lib,
  pkgs,
  ...
}:
let
  d = site.deployment;
  n = site.network;

  healthCheck = pkgs.writeShellApplication {
    name = "s13-health-check";
    # Self-contained on purpose: the updater runs this inside a service whose
    # PATH has no shell. `bash` must be a runtime input or every compound
    # check dies with command-not-found and reads as FAIL.
    runtimeInputs = with pkgs; [
      bash
      coreutils
      curl
      gnugrep
      iproute2
      iputils
    ];
    text = ''
      fail=0
      failed=""
      check() {
        local desc="$1"
        shift
        if "$@" >/dev/null 2>&1; then
          echo "ok:   $desc"
        else
          echo "FAIL: $desc"
          failed="''${failed}[$desc] "
          fail=$((fail + 1))
        fi
      }

      check "sshd active" systemctl is-active sshd
      check "${n.interfaceName} has ${n.address}" bash -c "ip -4 addr show ${n.interfaceName} | grep -qF 'inet ${n.address}/'"
      check "default route via ${n.interfaceName}" bash -c "ip route show default | grep -q 'dev ${n.interfaceName}'"
      check "nftables input policy loaded" bash -c "nft list ruleset | grep -q 'collector:api'"
      check "collector active" systemctl is-active ahara-collector
      check "collector API healthy" curl -sf --connect-timeout 3 http://${n.address}:${toString site.api.port}/health
      check "router reachable" ping -c 1 -W 2 ${n.routerIp}

      if [ "$fail" -eq 0 ]; then
        echo "health: all checks ok"
        exit 0
      fi
      echo "health: $fail failing: $failed"
      exit 1
    '';
  };

  updateScript = pkgs.writeShellApplication {
    name = "s13-update";
    runtimeInputs = with pkgs; [
      git
      gawk
      gnugrep
      nix
      openssh
    ];
    text = ''
      state_dir=/var/lib/s13-update
      values=/var/lib/ahara-collector/site-values.json
      mkdir -p "$state_dir"

      # The repo carries the design with placeholder values; this machine's
      # real values are host state, overlaid at build time.
      [ -r "$values" ] || {
        echo "no $values on this host; refusing to build placeholder values"
        exit 1
      }

      # Private-repo access is one explicit contract: the read-only deploy key
      # minted on first boot and registered in GitHub.
      deploy_key=/var/lib/ahara-collector/deploy-key
      [ -r "$deploy_key" ] || {
        echo "no $deploy_key on this host; register the generated deploy key"
        exit 1
      }
      export GIT_SSH_COMMAND="ssh -i $deploy_key -o IdentitiesOnly=yes"
      repo_url="${d.repoUrl}"
      repo_url="git@github.com:''${repo_url#https://github.com/}"

      target=$(git ls-remote "$repo_url" "refs/heads/${d.branch}" | awk '{print $1}')
      if [ -z "$target" ]; then
        echo "no ${d.branch} ref published yet; nothing to do"
        exit 0
      fi
      echo "$target" | grep -Eq '^[0-9a-f]{40}$' || {
        echo "refusing non-SHA release ref: $target"
        exit 1
      }

      # The change key is the pair (revision, values hash): a values-only edit
      # redeploys just like a new release.
      values_hash=$(sha256sum "$values" | cut -c1-16)
      desired="$target-$values_hash"
      current=$(cat "$state_dir/current" 2>/dev/null || echo none)
      if [ "$desired" = "$current" ]; then
        exit 0
      fi

      echo "building release $target with host values $values_hash (current: $current)"
      workdir=$(mktemp -d /var/tmp/s13-update.XXXXXX)
      trap 'rm -rf "$workdir"' EXIT
      git clone --quiet --depth 1 "$repo_url" "$workdir/repo" 2>/dev/null || \
        git clone --quiet "$repo_url" "$workdir/repo"
      git -C "$workdir/repo" fetch --quiet --depth 1 origin "$target"
      git -C "$workdir/repo" -c advice.detachedHead=false checkout --quiet "$target"
      rm -rf "$workdir/repo/.git"
      install -m 0644 "$values" "$workdir/repo/hosts/s13/site-values.json"

      next=$(nix build --no-link --print-out-paths \
        "path:$workdir/repo#nixosConfigurations.${site.host.name}.config.system.build.toplevel")
      prev=$(readlink -f /run/current-system)

      if [ "$next" = "$prev" ]; then
        echo "$desired" >"$state_dir/current"
        echo "system unchanged; recorded $desired"
        exit 0
      fi

      echo "activating $next"
      if ! "$next/bin/switch-to-configuration" test; then
        echo "activation failed; rolling back to $prev"
        "$prev/bin/switch-to-configuration" test
        exit 1
      fi

      # Health checks gate the deploy: a failing check restores the previous
      # generation and leaves the release uncommitted, so the next poll
      # retries it.
      if ! "$next/sw/bin/s13-health-check"; then
        echo "health check failed; rolling back to $prev"
        "$prev/bin/switch-to-configuration" test
        exit 1
      fi

      nix-env --profile /nix/var/nix/profiles/system --set "$next"
      "$next/bin/switch-to-configuration" boot
      echo "$desired" >"$state_dir/current"
      echo "deployed $desired"
    '';
  };
in
{
  environment.systemPackages = [ healthCheck ];

  # Read-only repo access for pull-deploys against a private repo: a deploy
  # key minted on first boot. Register the public half as a GitHub deploy key.
  systemd.services.s13-deploy-keygen = {
    description = "Generate the repo deploy key on first boot";
    wantedBy = [ "multi-user.target" ];
    unitConfig.ConditionPathExists = "!/var/lib/ahara-collector/deploy-key";
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      UMask = "0077";
    };
    path = [ pkgs.openssh ];
    script = ''
      mkdir -p /var/lib/ahara-collector
      ssh-keygen -t ed25519 -N "" -C "s13-deploy" -f /var/lib/ahara-collector/deploy-key
      chmod 644 /var/lib/ahara-collector/deploy-key.pub
    '';
  };

  # GitHub's published host key, pinned declaratively so the updater's ssh
  # never prompts.
  programs.ssh.knownHosts."github.com".publicKey =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

  systemd.services.s13-update = {
    description = "Fetch and activate the latest validated collector release";
    after = [ "network-online.target" ];
    wants = [ "network-online.target" ];
    # The service calls switch-to-configuration itself. Without both guards,
    # that switch stops this unit mid-transaction, killing the activator after
    # old units have stopped but before their replacements have started. These
    # are the same self-upgrade guards used by NixOS's nixos-upgrade service.
    restartIfChanged = false;
    unitConfig.X-StopOnRemoval = false;
    serviceConfig = {
      Type = "oneshot";
      ExecStart = lib.getExe updateScript;
    };
  };

  systemd.timers.s13-update = {
    wantedBy = [ "timers.target" ];
    timerConfig = {
      OnBootSec = "3min";
      OnUnitActiveSec = "${toString d.pollIntervalMinutes}min";
      RandomizedDelaySec = "30s";
    };
  };
}

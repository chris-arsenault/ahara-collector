# Pull-based self-deployment, the ahara-vpn ADR-0001/ADR-0008 pattern. A
# timer polls the release branch; on a new revision (or changed machine values)
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
    name = "collector-health-check";
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
      nftables
    ];
    text = ''
      fail=0
      failed=""

      # Bounded wait so a check whose subject legitimately arrives a moment
      # after activation judges a settled system. Called before the check it
      # precedes, never through check(), which would hide the call.
      settle() {
        local attempts="$1"
        shift
        local i=0
        while [ "$i" -lt "$attempts" ]; do
          if "$@" >/dev/null 2>&1; then
            return 0
          fi
          sleep 1
          i=$((i + 1))
        done
        return 1
      }

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
      # Consumers reach the API only through the terminator, so a release
      # that cannot serve TLS rolls back. The health check uses the numeric
      # address and checks availability rather than public-chain validation;
      # nginx binds an address networkd may hand over a moment late.
      tls_health="https://${n.address}:${toString site.api.tlsPort}/health"
      settle 15 curl -skf --connect-timeout 3 "$tls_health" || true
      check "API terminator healthy" curl -skf --connect-timeout 3 "$tls_health"
      check "router reachable" ping -c 1 -W 2 ${n.routerIp}

      if [ "$fail" -eq 0 ]; then
        echo "health: all checks ok"
        exit 0
      fi
      echo "health: $fail failing: $failed"
      exit 1
    '';
  };

  migrateConfig = pkgs.writeShellApplication {
    name = "collector-config-migrate";
    runtimeInputs = with pkgs; [
      coreutils
      jq
    ];
    text = ''
      dir=/var/lib/ahara-collector
      legacy="$dir/site-values.json"
      machine="$dir/machine-values.json"
      mkdir -p "$dir"

      if [ -r "$machine" ]; then
        exit 0
      fi
      [ -r "$legacy" ] || {
        echo "missing $machine and no legacy $legacy is available"
        exit 1
      }

      tmp=$(mktemp "$dir/.machine-values.json.XXXXXX")
      jq -e '{
        _comment: "Machine facts migrated from the former combined collector store.",
        interfaceMac: .network.interfaceMac,
        adminAuthorizedKeys: .adminAuthorizedKeys
      } | select(.interfaceMac != null and .adminAuthorizedKeys != null)' \
        "$legacy" >"$tmp"
      chmod 0644 "$tmp"
      mv "$tmp" "$machine"

      if [ ! -e "$legacy.migrated" ]; then
        mv "$legacy" "$legacy.migrated"
        echo "extracted collector machine facts; archived the legacy store at $legacy.migrated"
      else
        echo "extracted collector machine facts; existing archive retained and $legacy left untouched"
      fi
    '';
  };

  updateScript = pkgs.writeShellApplication {
    name = "collector-update";
    runtimeInputs = with pkgs; [
      git
      gawk
      gnugrep
      nix
    ];
    text = ''
      state_dir=/var/lib/collector-update
      machine=/var/lib/ahara-collector/machine-values.json
      mkdir -p "$state_dir"

      ${lib.getExe migrateConfig}

      # The repo carries versioned topology and placeholder machine facts;
      # this host's real hardware/access identity is overlaid at build time.
      [ -r "$machine" ] || {
        echo "no $machine on this host; refusing to build placeholder machine facts"
        exit 1
      }

      # The repo is public: the updater fetches anonymously over https, so
      # the appliance holds no repo credential at all.
      repo_url="${d.repoUrl}"

      target=$(git ls-remote "$repo_url" "refs/heads/${d.branch}" | awk '{print $1}')
      if [ -z "$target" ]; then
        echo "no ${d.branch} ref published yet; nothing to do"
        exit 0
      fi
      echo "$target" | grep -Eq '^[0-9a-f]{40}$' || {
        echo "refusing non-SHA release ref: $target"
        exit 1
      }

      # The change key is the pair (revision, machine hash): a hardware/access
      # identity edit redeploys just like a new release.
      machine_hash=$(sha256sum "$machine" | cut -c1-16)
      desired="$target-$machine_hash"
      current=$(cat "$state_dir/current" 2>/dev/null || echo none)
      if [ "$desired" = "$current" ]; then
        exit 0
      fi

      echo "building release $target with machine values $machine_hash (current: $current)"
      workdir=$(mktemp -d /var/tmp/collector-update.XXXXXX)
      trap 'rm -rf "$workdir"' EXIT
      git clone --quiet --depth 1 "$repo_url" "$workdir/repo" 2>/dev/null || \
        git clone --quiet "$repo_url" "$workdir/repo"
      git -C "$workdir/repo" fetch --quiet --depth 1 origin "$target"
      git -C "$workdir/repo" -c advice.detachedHead=false checkout --quiet "$target"
      rm -rf "$workdir/repo/.git"
      install -m 0644 "$machine" "$workdir/repo/hosts/collector/machine-values.json"

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
      if ! "$next/sw/bin/collector-health-check"; then
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

  systemd.services.collector-config-migrate = {
    description = "Split legacy collector configuration into machine-local state";
    wantedBy = [ "multi-user.target" ];
    before = [ "collector-update.service" ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      ExecStart = lib.getExe migrateConfig;
    };
  };

  systemd.services.collector-update = {
    description = "Fetch and activate the latest validated collector release";
    after = [
      "network-online.target"
      "collector-config-migrate.service"
    ];
    wants = [
      "network-online.target"
      "collector-config-migrate.service"
    ];
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

  systemd.timers.collector-update = {
    wantedBy = [ "timers.target" ];
    timerConfig = {
      OnBootSec = "3min";
      OnUnitActiveSec = "${toString d.pollIntervalMinutes}min";
      RandomizedDelaySec = "30s";
    };
  };
}

# The client an appliance runs to obtain and keep a machine identity.
#
# MIRRORED from ahara-trust:modules/enroll.nix. Keep the copies identical;
# they speak a protocol, and a divergence is a machine that silently stops
# renewing. A flake input would give one copy, but ahara-trust is private, so
# every appliance and every CI run would need a deploy key for a second
# repository — recurring credential provisioning traded against a file to
# keep in step. If ahara-trust ever becomes fetchable without credentials,
# collapse these back into one input.
#
# It is deliberately shell over curl and openssl: both are already on every
# appliance, mutual TLS is one flag, and nothing here needs to parse a
# certificate. The design constraint is that it must be safe to run on a timer
# forever — a device whose authority is unreachable, or whose id has not been
# declared there, exits quietly and tries again.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.ahara.enroll;

  client = pkgs.writeShellApplication {
    name = "ahara-enroll";
    runtimeInputs = with pkgs; [
      coreutils
      curl
      openssl
      jq
    ];
    text = ''
      state=${cfg.stateDir}
      key="$state/identity.key"
      cert="$state/identity.crt"
      chain="$state/authority.pem"
      url=${cfg.authorityUrl}
      workload=${cfg.workloadId}

      mkdir -p "$state"
      chmod 0700 "$state"

      # Fetching the CA is the one call with nothing to verify against, so it
      # is trust-on-first-use and -k. Everything afterwards is verified: the
      # authority terminates TLS with a certificate signed by this same CA,
      # so the pinned copy checks the transport as well as what comes back.
      if [ ! -s "$chain" ]; then
        curl -skf --max-time 10 "$url/ca.pem" -o "$chain" || {
          echo "authority unreachable; will retry"
          exit 0
        }
        openssl x509 -in "$chain" -noout -subject >/dev/null 2>&1 || {
          echo "authority returned no usable certificate; will retry"
          rm -f "$chain"
          exit 0
        }
      fi

      # Verify the transport against the pinned CA from here on.
      verified=(--cacert "$chain")

      renew_after() {
        # Half the certificate's life. An appliance off for less than that
        # recovers by itself; one off for longer falls back to enrolling
        # afresh, which needs nobody because its id is still declared.
        local not_after now half
        not_after=$(date -d "$(openssl x509 -in "$cert" -noout -enddate | cut -d= -f2)" +%s)
        not_before=$(date -d "$(openssl x509 -in "$cert" -noout -startdate | cut -d= -f2)" +%s)
        now=$(date +%s)
        half=$(( not_before + (not_after - not_before) / 2 ))
        [ "$now" -ge "$half" ]
      }

      request() {
        # A fresh key every time: renewal rotates the key, so a copy of the
        # old one is worth nothing once the new certificate is issued.
        # A fixed subject, not the workload id: openssl reads "/" in -subj as
        # the field separator, so a SPIFFE URI cannot go here. It would be
        # discarded regardless — the authority sets the subject and the SAN
        # itself and honours nothing from the request but its public key.
        openssl req -new -newkey rsa:3072 -nodes \
          -keyout "$state/next.key" -out "$state/next.csr" \
          -subj "/CN=ahara-machine" 2>/dev/null
        chmod 0600 "$state/next.key"
        jq -Rs --arg id "$workload" '{workload_id:$id, csr_pem:.}' \
          < "$state/next.csr" > "$state/next.json"
      }

      install_issued() {
        jq -e -r .certificate_pem < "$state/response.json" > "$state/next.crt"
        # The one check that makes the rest safe: an identity is installed
        # only if it chains to the authority pinned on first contact. A
        # substituted endpoint can refuse or stall, but cannot mint an
        # identity this machine will accept.
        if ! openssl verify -CAfile "$chain" "$state/next.crt" >/dev/null 2>&1; then
          echo "issued certificate does not chain to the pinned authority; discarding"
          rm -f "$state/next.crt" "$state/response.json"
          return 1
        fi
        mv "$state/next.key" "$key"
        mv "$state/next.crt" "$cert"
        chmod 0600 "$key"
        chmod 0644 "$cert"
        rm -f "$state/next.csr" "$state/next.json" "$state/response.json"
        echo "identity issued for $workload"
      }

      if [ -s "$cert" ] && [ -s "$key" ]; then
        renew_after || exit 0
        request
        # Renewal authenticates with the certificate already held, so no
        # operator is involved and no secret is carried.
        if curl -sf "''${verified[@]}" --cert "$cert" --key "$key" \
             --max-time 30 -X POST --data-binary @"$state/next.json" \
             "$url/renew" -o "$state/response.json"; then
          install_issued
        else
          echo "renewal refused or authority unreachable; keeping current identity"
        fi
        exit 0
      fi

      # No identity yet. The authority issues to any id declared in its site
      # policy, so this either succeeds or is refused outright; there is
      # nothing to wait for and nobody to ask.
      [ -s "$state/next.json" ] || request
      code=$(curl -s "''${verified[@]}" --max-time 30 -o "$state/response.json" \
        -w '%{http_code}' -X POST --data-binary @"$state/next.json" "$url/enroll" || echo 000)

      # A refusal is permanent until someone changes the authority's policy,
      # so the unit fails and stays visible in `systemctl --failed`. Exiting
      # 0 here would make a machine that will never hold an identity look
      # healthy forever. Unreachable is transient, so that one exits clean.
      case "$code" in
        200) install_issued ;;
        000) echo "authority unreachable; will retry" ;;
        403) echo "$workload is not declared on the authority; declare it there"; exit 1 ;;
        *)   echo "enrollment refused (HTTP $code)"; exit 1 ;;
      esac
    '';
  };
in
{
  options.ahara.enroll = {
    enable = lib.mkEnableOption "machine identity enrollment against the trust appliance";

    authorityUrl = lib.mkOption {
      type = lib.types.str;
      description = "Base URL of the trust appliance's enrollment API.";
    };

    workloadId = lib.mkOption {
      type = lib.types.str;
      description = "This machine's identity, as a SPIFFE URI.";
    };

    stateDir = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/ahara-identity";
      description = "Where the key, certificate, and pinned authority live.";
    };

    intervalMinutes = lib.mkOption {
      type = lib.types.int;
      default = 60;
      description = "How often to request, collect, or renew.";
    };

    certificate = {
      enable = lib.mkEnableOption "fetching the shared publicly-trusted certificate";

      destination = lib.mkOption {
        type = lib.types.str;
        description = "Where to write the fetched certificate.";
      };

      keyDestination = lib.mkOption {
        type = lib.types.str;
        description = "Where to write its private key.";
      };

      owner = lib.mkOption {
        type = lib.types.str;
        default = "root:nginx";
        description = "Owner of the key; the terminator's user must read it.";
      };

      reloadUnits = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ "nginx.service" ];
        description = "Units to reload when the certificate changes.";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ client ];

    # Created before the unit runs: the sandbox binds this path, and systemd
    # sets the namespace up before the script that would have made it.
    systemd.tmpfiles.rules = [ "d ${cfg.stateDir} 0700 root root -" ];

    systemd.services.ahara-enroll = {
      description = "Obtain and renew this machine's identity";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      serviceConfig = {
        Type = "oneshot";
        ExecStart = lib.getExe client;
        # The private key is the machine's identity; nothing but root reads it.
        UMask = "0077";
        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        ReadWritePaths = [ cfg.stateDir ];
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        LockPersonality = true;
      };
    };

    systemd.timers.ahara-enroll = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnBootSec = "2min";
        OnUnitActiveSec = "${toString cfg.intervalMinutes}min";
        RandomizedDelaySec = "60s";
      };
    };

    # Fetching the shared certificate rides the same identity and the same
    # timer: a machine that can prove who it is may have the certificate, and
    # one that cannot keeps whatever it is already serving.
    systemd.services.ahara-certificate = lib.mkIf cfg.certificate.enable {
      description = "Fetch the shared publicly-trusted certificate";
      after = [
        "network-online.target"
        "ahara-enroll.service"
      ];
      wants = [ "network-online.target" ];
      serviceConfig = {
        Type = "oneshot";
        ExecStart = lib.getExe (
          pkgs.writeShellApplication {
            name = "ahara-certificate";
            runtimeInputs = with pkgs; [
              coreutils
              curl
              jq
              systemd
            ];
            text = ''
              state=${cfg.stateDir}
              cert=${cfg.certificate.destination}
              key=${cfg.certificate.keyDestination}

              [ -s "$state/identity.crt" ] || {
                echo "no machine identity yet; nothing to authenticate with"
                exit 0
              }

              tmp=$(mktemp -d)
              trap 'rm -rf "$tmp"' EXIT

              # The authority terminates TLS with a certificate from the CA
              # pinned here when the identity was obtained, so the transport
              # is verified rather than taken on faith.
              code=$(curl -s --cacert "$state/authority.pem" \
                --cert "$state/identity.crt" --key "$state/identity.key" \
                --max-time 30 -o "$tmp/response.json" -w '%{http_code}' \
                ${cfg.authorityUrl}/certificate || echo 000)

              case "$code" in
                200) ;;
                404) echo "authority has no certificate yet; keeping current"; exit 0 ;;
                000) echo "authority unreachable; keeping current"; exit 0 ;;
                *)   echo "refused (HTTP $code); keeping current"; exit 0 ;;
              esac

              jq -e -r .certificate_pem < "$tmp/response.json" > "$tmp/fullchain.pem"
              jq -e -r .private_key_pem < "$tmp/response.json" > "$tmp/privkey.pem"

              # Replace only on change: reloading the terminator on every
              # tick would be churn, and comparing is cheaper than reloading.
              if [ -s "$cert" ] && cmp -s "$tmp/fullchain.pem" "$cert"; then
                exit 0
              fi

              mkdir -p "$(dirname "$cert")" "$(dirname "$key")"
              install -m 0644 "$tmp/fullchain.pem" "$cert"
              install -m 0640 -o "''${OWNER%%:*}" -g "''${OWNER##*:}" "$tmp/privkey.pem" "$key"
              echo "installed a new shared certificate"
              ${lib.concatMapStringsSep "\n" (
                unit: "systemctl reload-or-restart ${unit} || true"
              ) cfg.certificate.reloadUnits}
            '';
          }
        );
        Environment = [ "OWNER=${cfg.certificate.owner}" ];
        UMask = "0077";
      };
    };

    systemd.timers.ahara-certificate = lib.mkIf cfg.certificate.enable {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnBootSec = "4min";
        OnUnitActiveSec = "${toString cfg.intervalMinutes}min";
        RandomizedDelaySec = "60s";
      };
    };
  };
}

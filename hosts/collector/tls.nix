# TLS termination for the pull API. The collector service speaks plain HTTP
# and stays that way — it is a dependency-free binary whose hand-written HTTP
# parsing has no business holding a private key — so nginx fronts it on the
# appliance's own address: consumers connect to collector.local.ahara.io and
# verify a publicly-trusted chain, and the plaintext leg never leaves this
# host's network stack.
#
# The certificate is issued and renewed here via Route53 DNS-01 (ahara-vpn
# ADR-0015). Its credential is host state; without it the ACME order is
# skipped and nginx serves the module's self-signed placeholder, so a missing
# or broken issuance path degrades TLS trust and nothing else.
{
  site,
  lib,
  pkgs,
  ...
}:
let
  n = site.network;
  api = site.api;
in
{
  services.nginx = {
    enable = true;
    recommendedTlsSettings = true;
    recommendedProxySettings = true;
    virtualHosts.${api.hostName} = {
      default = true;
      # The module emits ssl_certificate only for a vhost that declares SSL;
      # a listener's `ssl = true` alone leaves nginx failing its config test.
      onlySSL = true;
      listen = [
        {
          addr = n.address;
          port = api.tlsPort;
          ssl = true;
        }
      ];
      useACMEHost = api.hostName;
      locations."/" = {
        proxyPass = "http://${n.address}:${toString api.port}";
        # Drains are one spool segment; the default 60s read timeout is
        # ample, but streaming responses must not be buffered to disk.
        extraConfig = ''
          proxy_buffering off;
        '';
      };
    };
  };

  systemd.services.nginx = {
    # nginx binds the appliance address explicitly, and a bind attempted
    # before networkd has assigned it fails outright. Retry without a start
    # limit so boot ordering cannot leave the API terminator down.
    unitConfig.StartLimitIntervalSec = lib.mkForce 0;
    serviceConfig.RestartSec = lib.mkForce "2s";
  };

  security.acme = {
    acceptTerms = true;
    defaults.email = api.acme.email;
    certs.${api.hostName} = {
      dnsProvider = "route53";
      environmentFile = api.acme.credentialsFile;
      # Route53's change-INSYNC wait already proves the TXT record is live;
      # the appliance's own resolver is the gateway, which is authoritative
      # for the internal subtree and never sees the public record.
      dnsPropagationCheck = false;
      group = "nginx";
      reloadServices = [ "nginx" ];
    };
  };

  # Gate the ordering unit, never `acme-${hostName}.service`: that one
  # generates the self-signed placeholder nginx loads until a real
  # certificate exists, so skipping it leaves nginx with no certificate at
  # all and the API unreachable. Only this unit consumes the credential.
  # Installing the file and starting it (or waiting for its timer) performs
  # the first issuance.
  systemd.services."acme-order-renew-${api.hostName}".unitConfig.ConditionPathExists =
    api.acme.credentialsFile;

  # Certificate expiry as a metric on the API's own /metrics surface would
  # need the service to read it; the appliance exports it the same way the
  # gateway does instead — a textfile the health check reads.
  systemd.services.tls-cert-metrics = {
    description = "Export the pull API certificate expiry";
    serviceConfig.Type = "oneshot";
    path = [
      pkgs.coreutils
      pkgs.openssl
    ];
    script = ''
      dir=/var/lib/ahara-collector/metrics
      mkdir -p "$dir"
      cert="/var/lib/acme/${api.hostName}/fullchain.pem"
      tmp=$(mktemp "$dir/.tls_cert.prom.XXXXXX")
      {
        if [ -r "$cert" ]; then
          end=$(openssl x509 -in "$cert" -noout -enddate | cut -d= -f2)
          echo "tls_cert_not_after_seconds{name=\"${api.hostName}\"} $(date -d "$end" +%s)"
        else
          echo "tls_cert_not_after_seconds{name=\"${api.hostName}\"} 0"
        fi
      } > "$tmp"
      chmod 0644 "$tmp"
      mv "$tmp" "$dir/tls_cert.prom"
    '';
  };

  systemd.timers.tls-cert-metrics = {
    wantedBy = [ "timers.target" ];
    timerConfig = {
      OnBootSec = "2min";
      OnUnitActiveSec = "1h";
    };
  };
}

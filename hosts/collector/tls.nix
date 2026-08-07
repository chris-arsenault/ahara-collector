# TLS termination for the pull API. The collector service speaks plain HTTP
# and stays that way — it is a dependency-free binary whose hand-written HTTP
# parsing has no business holding a private key — so nginx fronts it on the
# appliance's own address: consumers connect to collector.local.ahara.io and
# verify a publicly-trusted chain, and the plaintext leg never leaves this
# host's network stack.
#
# The certificate is self-signed on first boot and replaced by one the
# machine-identity appliance obtains and distributes (ADR-0008). This
# appliance runs no ACME client and holds no cloud credential of any kind.
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
      sslCertificate = api.certificate;
      sslCertificateKey = api.certificateKey;
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
    after = [ "ahara-collector-tls.service" ];
    requires = [ "ahara-collector-tls.service" ];
    # nginx binds the appliance address explicitly, and a bind attempted
    # before networkd has assigned it fails outright. Retry without a start
    # limit so boot ordering cannot leave the API terminator down.
    unitConfig.StartLimitIntervalSec = lib.mkForce 0;
    serviceConfig.RestartSec = lib.mkForce "2s";
  };

  # Self-signed until the machine-identity appliance distributes a
  # publicly-trusted certificate for this name. This appliance runs no ACME
  # client and holds no cloud credential.
  systemd.services.ahara-collector-tls = {
    description = "Generate the pull API TLS certificate on first boot";
    wantedBy = [ "multi-user.target" ];
    unitConfig.ConditionPathExists = "!${api.certificate}";
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
    };
    path = [
      pkgs.coreutils
      pkgs.openssl
    ];
    script = ''
      mkdir -p "$(dirname ${api.certificate})"
      openssl req -x509 -newkey rsa:4096 -sha256 -nodes -days 3650 \
        -subj "/CN=${api.hostName}" \
        -addext "subjectAltName=DNS:${api.hostName},IP:${n.address}" \
        -keyout ${api.certificateKey} -out ${api.certificate}
      chown root:nginx ${api.certificateKey}
      chmod 640 ${api.certificateKey}
      chmod 644 ${api.certificate}
    '';
  };

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
      cert="${api.certificate}"
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

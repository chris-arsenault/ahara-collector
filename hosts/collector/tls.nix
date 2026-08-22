# TLS termination for the pull API. The collector service speaks plain HTTP
# and stays that way — its device-protocol clients have no business holding
# the API's private key — so nginx fronts it on the
# appliance's own address: consumers connect to collector.local.ahara.io and
# verify a publicly-trusted chain, and the plaintext leg never leaves this
# host's network stack.
#
# The certificate comes only from the machine-identity appliance (ADR-0008).
# This appliance generates no stand-in, runs no ACME client, and holds no
# cloud credential of any kind.
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
  # Declared rather than left to whichever unit calls mkdir first. Certificate
  # distribution runs under a restrictive umask, and the terminator has to walk
  # this path to reach the certificate it serves; a directory whose mode
  # depends on boot ordering is a surface that comes up or does not for reasons
  # nothing reports. The private key beside it carries its own mode.
  systemd.tmpfiles.rules = [
    "d ${builtins.dirOf api.certificate} 0755 root root -"
  ];

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

  # The certificate comes from the machine-identity appliance and there is no
  # locally generated stand-in. Without one nginx does not start, the health
  # check fails, and the deploy rolls back — an appliance that cannot obtain
  # its certificate is misconfigured, and a placeholder would hide that.
  #
  # No ConditionPathExists on the certificate: a skipped unit is quiet, and
  # nginx failing to read one that is not there is the signal.
  systemd.services.nginx = {
    # ahara-certificate installs it, having first obtained an identity to
    # fetch it with. Both are timer-driven, so they are pulled in here to run
    # at boot rather than at the timer's first elapse.
    after = [ "ahara-certificate.service" ];
    wants = [ "ahara-certificate.service" ];
    # nginx binds the appliance address explicitly, and a bind attempted
    # before networkd has assigned it fails outright. Retry without a start
    # limit so boot ordering cannot leave the API terminator down.
    unitConfig.StartLimitIntervalSec = lib.mkForce 0;
    serviceConfig.RestartSec = lib.mkForce "2s";
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

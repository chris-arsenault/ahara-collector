# The collector service: SSDP relay for Airwave, device pollers, bounded
# spool, and the single-port pull API. All topology arrives as one JSON
# config document rendered from site.nix; secrets never do — the device
# credentials file and the scoped API tokens are host state passed through systemd
# credentials, invisible to the store and the environment.
{
  pkgs,
  site,
  collectorPackage,
  ...
}:
let
  n = site.network;
  c = site.collector;
  stateDir = "/var/lib/ahara-collector";

  configJson = pkgs.writeText "ahara-collector-config.json" (
    builtins.toJSON {
      bindAddress = n.address;
      homeCidr = n.homeLanCidr;
      homeBroadcast = n.homeBroadcast;
      apiPort = site.api.port;
      airwaveSsdp = c.airwaveSsdp;
      wiim = c.wiim;
      envSensors = c.envSensors;
      kasa = c.kasa;
      spool = c.spool;
    }
  );
in
{
  environment.systemPackages = [ collectorPackage ];

  # Host state the service depends on but must not own: the device
  # credentials file (scp'd or pasted by the operator, empty until then) and
  # scoped API bearer tokens (generated once at first boot).
  systemd.tmpfiles.rules = [
    "d ${stateDir} 0750 root root -"
    "f ${stateDir}/credentials.json 0600 root root -"
  ];

  systemd.services.ahara-collector-token = {
    description = "Generate the House Sensors collector bearer token on first boot";
    wantedBy = [ "multi-user.target" ];
    unitConfig.ConditionPathExists = "!${stateDir}/api-token";
    serviceConfig = {
      Type = "oneshot";
      # Stays active after completion: the collector service Requires this
      # unit, and a plain oneshot reads as inactive once it has run.
      RemainAfterExit = true;
      UMask = "0077";
    };
    path = [ pkgs.coreutils ];
    script = ''
      head -c 96 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | cut -c1-48 \
        > ${stateDir}/api-token
      chmod 600 ${stateDir}/api-token
      echo "House Sensors token generated; read it with: sudo cat ${stateDir}/api-token"
    '';
  };

  systemd.services.ahara-collector-airwave-token = {
    description = "Generate the Airwave collector bearer token on first boot";
    wantedBy = [ "multi-user.target" ];
    unitConfig.ConditionPathExists = "!${stateDir}/airwave-token";
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      UMask = "0077";
    };
    path = [ pkgs.coreutils ];
    script = ''
      head -c 96 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | cut -c1-48 \
        > ${stateDir}/airwave-token
      chmod 600 ${stateDir}/airwave-token
      echo "Airwave token generated; read it with: sudo cat ${stateDir}/airwave-token"
    '';
  };

  systemd.services.ahara-collector = {
    description = "Ahara IoT collector (SSDP relay, device pollers, pull API)";
    wantedBy = [ "multi-user.target" ];
    requires = [
      "ahara-collector-token.service"
      "ahara-collector-airwave-token.service"
    ];
    after = [
      "network-online.target"
      "nftables.service"
      "ahara-collector-token.service"
      "ahara-collector-airwave-token.service"
    ];
    wants = [ "network-online.target" ];

    serviceConfig = {
      ExecStart = "${pkgs.lib.getExe collectorPackage} run --config ${configJson} --token-file \${CREDENTIALS_DIRECTORY}/api-token --airwave-token-file \${CREDENTIALS_DIRECTORY}/airwave-token --credentials \${CREDENTIALS_DIRECTORY}/devices.json";
      LoadCredential = [
        "api-token:${stateDir}/api-token"
        "airwave-token:${stateDir}/airwave-token"
        "devices.json:${stateDir}/credentials.json"
      ];
      Restart = "on-failure";
      RestartSec = "2s";

      DynamicUser = true;
      StateDirectory = [
        "ahara-collector-spool"
        "ahara-collector-runtime"
      ];
      NoNewPrivileges = true;
      PrivateDevices = true;
      PrivateTmp = true;
      ProtectSystem = "strict";
      ProtectHome = true;
      ProtectKernelTunables = true;
      ProtectKernelModules = true;
      ProtectControlGroups = true;
      RestrictAddressFamilies = [ "AF_INET" ];
      RestrictNamespaces = true;
      LockPersonality = true;
      MemoryDenyWriteExecute = true;
      CapabilityBoundingSet = [ ];
      SystemCallFilter = [
        "@system-service"
        "~@privileged"
        "~@resources"
      ];
    };
  };
}

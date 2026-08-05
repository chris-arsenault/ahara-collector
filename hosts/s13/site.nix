# Single source of truth for the S13 collector appliance. Machine-specific
# inputs live in site-values.json (placeholders in git; a real machine's store
# is host state overlaid at build time, the ahara-vpn ADR-0006 pattern);
# everything here derives from those values or is a topology constant, so no
# value is ever declared twice. lib/site-assertions.nix validates the result
# at evaluation time.
#
#   import ./site.nix { }                      -> production site
#   import ./site.nix { values = ...; }        -> test variant
{
  values ? removeAttrs (builtins.fromJSON (builtins.readFile ./site-values.json)) [ "_comment" ],
}:
let
  v = values;
  lib = import ../../lib/site-assertions.nix;
  n = v.network;
  homeBroadcast = lib.broadcastOf n.homeLanCidr;
  prefixLength = (lib.parseCidr n.homeLanCidr).prefix;
in
{
  host = {
    name = v.hostName;
    stateVersion = v.stateVersion;
    adminAuthorizedKeys = v.adminAuthorizedKeys;
  };

  network = {
    inherit (n)
      interfaceMac
      homeLanCidr
      address
      routerIp
      dnsServers
      truenasIp
      ;
    inherit homeBroadcast prefixLength;
    # The appliance has exactly one network identity: a single interface on
    # the home LAN. Server-subnet traffic (Airwave SSDP in, readings pull in)
    # arrives routed through the VP2440 with its original source address, so
    # local firewall rules can pin flows to the TrueNAS address.
    interfaceName = "lan0";
  };

  deployment = {
    inherit (v.deployment) repoUrl branch pollIntervalMinutes;
  };

  api = {
    inherit (v.api) port;
  };

  # Module configuration handed to the collector service as one JSON document
  # (no secrets in it — credentials are a separate host-state file loaded via
  # systemd credentials).
  collector = {
    airwaveSsdp = v.airwaveSsdp // {
      airwaveIp = n.truenasIp;
      ssdpPort = 1900;
      responsePort = 1901;
    };
    # Discovery sockets bind fixed local ports so the reply openings in the
    # input firewall stay narrow (broadcast requests cannot ride conntrack).
    envSensors = v.envSensors // {
      discoveryBindPort = 12344;
    };
    kasa = v.kasa // {
      discoveryBindPort = 20003;
    };
    spool = v.spool // {
      dir = "/var/lib/ahara-collector/spool";
    };
  };
}

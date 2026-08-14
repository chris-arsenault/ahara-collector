# Single composition point for the collector appliance. Infrequent topology
# and service changes live in topology.json and move through Git. Hardware and
# access identity live in machine-values.json on the host and are overlaid at
# build time. A former combined site-values.json is accepted only as migration
# input for those machine facts; its topology never overrides the release.
# lib/site-assertions.nix validates the composed result at evaluation time.
#
#   import ./site.nix { }                         -> production site
#   import ./site.nix { machineValues = ...; }    -> test hardware variant
{
  topology ? removeAttrs (builtins.fromJSON (builtins.readFile ./topology.json)) [ "_comment" ],
  legacyValues ?
    if builtins.pathExists ./site-values.json then
      removeAttrs (builtins.fromJSON (builtins.readFile ./site-values.json)) [ "_comment" ]
    else
      { },
  machineValues ? removeAttrs (builtins.fromJSON (builtins.readFile ./machine-values.json)) [
    "_comment"
  ],
}:
let
  # The legacy overlay lets the first release after this split build on a
  # host whose old updater still copied site-values.json into the checkout.
  # Only machine identity is read from it. topology.json wins immediately.
  legacyMachine =
    if legacyValues ? network && legacyValues.network ? interfaceMac then
      {
        interfaceMac = legacyValues.network.interfaceMac;
        adminAuthorizedKeys = legacyValues.adminAuthorizedKeys;
      }
    else
      { };
  machine = machineValues // legacyMachine;
  v = topology;
  lib = import ../../lib/site-assertions.nix;
  n = v.network;
  homeBroadcast = lib.broadcastOf n.homeLanCidr;
  prefixLength = (lib.parseCidr n.homeLanCidr).prefix;
  # The subtree the VP2440 is authoritative for. It is a real subdomain of
  # the owned domain, so this appliance's name carries a publicly-valid
  # certificate (ahara-vpn ADR-0015); the gateway serves the matching record.
  internalDomain = "local.ahara.io";
in
{
  host = {
    name = v.hostName;
    stateVersion = v.stateVersion;
    adminAuthorizedKeys = machine.adminAuthorizedKeys;
  };

  network = {
    inherit (n)
      homeLanCidr
      adminLanCidr
      address
      routerIp
      dnsServers
      truenasIp
      ;
    interfaceMac = machine.interfaceMac;
    inherit homeBroadcast prefixLength;
    # The appliance has exactly one network identity: a single interface on
    # the IoT LAN. Server-subnet traffic (Airwave SSDP in, readings pull in)
    # arrives routed through the VP2440 with its original source address, so
    # local firewall rules can pin flows to the TrueNAS address.
    interfaceName = "lan0";
  };

  deployment = {
    inherit (v.deployment) repoUrl branch pollIntervalMinutes;
  };

  api = {
    inherit (v.api) port;
    # Defaulted like every other key added after a machine was installed: an
    # existing store predating it keeps evaluating, so the appliance never
    # silently stops updating.
    tlsPort = v.api.tlsPort or 8443;
    # Consumers reach the API by this name; the plain port stays bound for
    # the not-yet-cut-over TrueNAS puller (docs/backlog.md).
    hostName = "collector.${internalDomain}";
    # Supplied only by the machine-identity appliance (ADR-0008). This
    # appliance generates no stand-in, runs no ACME client, and holds no cloud
    # credential.
    # Beside the state directory rather than inside it: that one is 0750 so
    # the device credentials and the API token stay unreadable, and nginx
    # cannot traverse it to load a certificate.
    certificate = "/var/lib/ahara-collector-tls/api.crt";
    certificateKey = "/var/lib/ahara-collector-tls/api.key";
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
    wiim = v.wiim // {
      ssdpPort = 1900;
      discoveryBindPort = 1902;
      stateFile = "/var/lib/ahara-collector-runtime/wiim-devices.json";
    };
    # Discovery sockets bind fixed local ports so the reply openings in the
    # input firewall stay narrow (broadcast requests cannot ride conntrack).
    envSensors = v.envSensors // {
      discoveryBindPort = 12344;
    };
    kasa = v.kasa // {
      discoveryBindPort = 20003;
    };
    # A standalone state directory owned entirely by the service's dynamic
    # user; /var/lib/ahara-collector stays root-owned host state (values,
    # credentials, keys) that the sandboxed service cannot traverse.
    spool = v.spool // {
      dir = "/var/lib/ahara-collector-spool";
    };
  };
}

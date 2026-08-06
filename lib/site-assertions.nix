# Validation for the S13 site attrset. Pure builtins — no nixpkgs — so tests
# and the flake can import it with zero dependencies. The API mirrors
# ahara-vpn's lib/network-assertions.nix: validateSite returns a list of
# human-readable errors (empty when valid); assertValid throws them. It
# validates the derived site (hosts/s13/site.nix output), not the raw values,
# so derivation mistakes fail evaluation too.
let
  inherit (builtins)
    all
    concatStringsSep
    div
    filter
    isAttrs
    isBool
    isInt
    isList
    isString
    length
    match
    split
    stringLength
    ;

  octet = "([0-9]|[1-9][0-9]|1[0-9][0-9]|2[0-4][0-9]|25[0-5])";
  ipRe = "${octet}\\.${octet}\\.${octet}\\.${octet}";

  isIp = s: isString s && match ipRe s != null;
  isCidr = s: isString s && match "${ipRe}/([0-9]|[12][0-9]|3[0-2])" s != null;
  isMac = s: isString s && match "([0-9a-f]{2}:){5}[0-9a-f]{2}" s != null;
  isSshKey = s: isString s && match "(ssh|sk-ssh)-[a-z0-9@.-]+ [A-Za-z0-9+/=]+( .*)?" s != null;

  mod = a: b: a - b * (div a b);

  toIntList = s: map builtins.fromJSON (filter (x: isString x && x != "") (split "\\." s));

  ipToInt =
    ip:
    let
      p = toIntList ip;
    in
    (builtins.elemAt p 0) * 16777216
    + (builtins.elemAt p 1) * 65536
    + (builtins.elemAt p 2) * 256
    + (builtins.elemAt p 3);

  intToIp =
    i:
    concatStringsSep "." (
      map toString [
        (div i 16777216)
        (mod (div i 65536) 256)
        (mod (div i 256) 256)
        (mod i 256)
      ]
    );

  pow2 = n: if n == 0 then 1 else 2 * pow2 (n - 1);

  parseCidr =
    cidr:
    let
      parts = split "/" cidr;
      base = builtins.elemAt parts 0;
      prefix = builtins.fromJSON (builtins.elemAt parts 2);
      size = pow2 (32 - prefix);
      network = div (ipToInt base) size * size;
    in
    {
      inherit prefix size network;
      first = network;
      last = network + size - 1;
    };

  ipInCidr =
    ip: cidr:
    let
      c = parseCidr cidr;
      i = ipToInt ip;
    in
    i >= c.first && i <= c.last;

  broadcastOf = cidr: intToIp (parseCidr cidr).last;

  getPath =
    site: path:
    builtins.foldl' (acc: key: if isAttrs acc && acc ? ${key} then acc.${key} else null) site path;

  isPort = p: isInt p && p >= 1 && p <= 65535;
  isPollSeconds = i: isInt i && i >= 1 && i <= 3600;
  isDiscoveryHours = i: isInt i && i >= 1 && i <= 168;

  # Structural contract: every entry names a dotted path, a predicate, and the
  # phrase used in its error message.
  requiredFields = [
    {
      path = [
        "host"
        "name"
      ];
      check = s: isString s && match "[a-z][a-z0-9-]*" s != null;
      describe = "lowercase hostname";
    }
    {
      path = [
        "host"
        "stateVersion"
      ];
      check = s: isString s && match "[0-9]+\\.[0-9]+" s != null;
      describe = "NixOS state version";
    }
    {
      path = [
        "host"
        "adminAuthorizedKeys"
      ];
      check = l: isList l && length l > 0 && all isSshKey l;
      describe = "non-empty list of SSH public keys";
    }
    {
      path = [
        "network"
        "interfaceMac"
      ];
      check = isMac;
      describe = "lowercase MAC address";
    }
    {
      path = [
        "network"
        "interfaceName"
      ];
      check = s: isString s && stringLength s > 0;
      describe = "interface name";
    }
    {
      path = [
        "network"
        "homeLanCidr"
      ];
      check = isCidr;
      describe = "IPv4 CIDR";
    }
    {
      path = [
        "network"
        "address"
      ];
      check = isIp;
      describe = "IPv4 address";
    }
    {
      path = [
        "network"
        "homeBroadcast"
      ];
      check = isIp;
      describe = "IPv4 address";
    }
    {
      path = [
        "network"
        "prefixLength"
      ];
      check = i: isInt i && i >= 1 && i <= 32;
      describe = "prefix length 1-32";
    }
    {
      path = [
        "network"
        "routerIp"
      ];
      check = isIp;
      describe = "IPv4 address";
    }
    {
      path = [
        "network"
        "dnsServers"
      ];
      check = l: isList l && length l > 0 && all isIp l;
      describe = "non-empty list of IPv4 addresses";
    }
    {
      path = [
        "network"
        "truenasIp"
      ];
      check = isIp;
      describe = "IPv4 address";
    }
    {
      path = [
        "deployment"
        "repoUrl"
      ];
      check = s: isString s && match "(https|git|ssh)://.*|git@.*" s != null;
      describe = "repository URL";
    }
    {
      path = [
        "deployment"
        "branch"
      ];
      check = s: isString s && stringLength s > 0;
      describe = "branch name";
    }
    {
      path = [
        "deployment"
        "pollIntervalMinutes"
      ];
      check = i: isInt i && i >= 1 && i <= 60;
      describe = "poll interval 1-60 minutes";
    }
    {
      path = [
        "api"
        "port"
      ];
      check = p: isInt p && p >= 1024 && p <= 65535;
      describe = "unprivileged port 1024-65535";
    }
    {
      path = [
        "api"
        "tlsPort"
      ];
      check = p: isInt p && p >= 1024 && p <= 65535;
      describe = "unprivileged port 1024-65535";
    }
    {
      path = [
        "api"
        "hostName"
      ];
      check = s: isString s && match "[a-zA-Z0-9.-]+" s != null;
      describe = "DNS name";
    }
    {
      path = [
        "api"
        "acme"
        "email"
      ];
      check = s: isString s && match ".+@.+" s != null;
      describe = "email address";
    }
    {
      path = [
        "api"
        "acme"
        "credentialsFile"
      ];
      check = s: isString s && match "/.+" s != null;
      describe = "absolute path";
    }
    {
      path = [
        "collector"
        "airwaveSsdp"
        "enable"
      ];
      check = isBool;
      describe = "boolean";
    }
    {
      path = [
        "collector"
        "airwaveSsdp"
        "airwaveIp"
      ];
      check = isIp;
      describe = "IPv4 address";
    }
    {
      path = [
        "collector"
        "airwaveSsdp"
        "ssdpPort"
      ];
      check = isPort;
      describe = "port 1-65535";
    }
    {
      path = [
        "collector"
        "airwaveSsdp"
        "responsePort"
      ];
      check = isPort;
      describe = "port 1-65535";
    }
    {
      path = [
        "collector"
        "airwaveSsdp"
        "relayPort"
      ];
      check = isPort;
      describe = "port 1-65535";
    }
    {
      path = [
        "collector"
        "airwaveSsdp"
        "responseWindowSeconds"
      ];
      check = i: isInt i && i >= 1 && i <= 10;
      describe = "response window 1-10 seconds";
    }
    {
      path = [
        "collector"
        "envSensors"
        "enable"
      ];
      check = isBool;
      describe = "boolean";
    }
    {
      path = [
        "collector"
        "envSensors"
        "discoveryPort"
      ];
      check = isPort;
      describe = "port 1-65535";
    }
    {
      path = [
        "collector"
        "envSensors"
        "devicePort"
      ];
      check = isPort;
      describe = "port 1-65535";
    }
    {
      path = [
        "collector"
        "envSensors"
        "pollIntervalSeconds"
      ];
      check = isPollSeconds;
      describe = "poll interval 1-3600 seconds";
    }
    {
      path = [
        "collector"
        "envSensors"
        "discoveryIntervalHours"
      ];
      check = isDiscoveryHours;
      describe = "discovery interval 1-168 hours";
    }
    {
      path = [
        "collector"
        "envSensors"
        "discoveryBindPort"
      ];
      check = isPort;
      describe = "port 1-65535";
    }
    {
      path = [
        "collector"
        "kasa"
        "discoveryBindPort"
      ];
      check = isPort;
      describe = "port 1-65535";
    }
    {
      path = [
        "collector"
        "kasa"
        "enable"
      ];
      check = isBool;
      describe = "boolean";
    }
    {
      path = [
        "collector"
        "kasa"
        "discoveryPort"
      ];
      check = isPort;
      describe = "port 1-65535";
    }
    {
      path = [
        "collector"
        "kasa"
        "devicePort"
      ];
      check = isPort;
      describe = "port 1-65535";
    }
    {
      path = [
        "collector"
        "kasa"
        "pollIntervalSeconds"
      ];
      check = isPollSeconds;
      describe = "poll interval 1-3600 seconds";
    }
    {
      path = [
        "collector"
        "kasa"
        "discoveryIntervalHours"
      ];
      check = isDiscoveryHours;
      describe = "discovery interval 1-168 hours";
    }
    {
      path = [
        "collector"
        "kasa"
        "staticDevices"
      ];
      check = l: isList l && all isIp l;
      describe = "list of IPv4 addresses";
    }
    {
      path = [
        "collector"
        "spool"
        "dir"
      ];
      check = s: isString s && match "/.*" s != null;
      describe = "absolute path";
    }
    {
      path = [
        "collector"
        "spool"
        "maxBytes"
      ];
      check = i: isInt i && i >= 1048576;
      describe = "spool cap of at least 1 MiB";
    }
    {
      path = [
        "collector"
        "spool"
        "segmentBytes"
      ];
      check = i: isInt i && i >= 65536;
      describe = "segment size of at least 64 KiB";
    }
  ];

  structureErrors =
    site:
    map (f: "site.${concatStringsSep "." f.path}: missing or not a ${f.describe}") (
      filter (
        f:
        let
          value = getPath site f.path;
        in
        value == null || !(f.check value)
      ) requiredFields
    );

  # Semantic checks run only once structure is clean, so they can assume the
  # fields exist and have the right shapes.
  semanticErrors =
    site:
    let
      n = site.network;
      c = site.collector;
    in
    (
      if ipInCidr n.address n.homeLanCidr then
        [ ]
      else
        [ "site.network.address must be inside network.homeLanCidr" ]
    )
    ++ (
      if ipInCidr n.routerIp n.homeLanCidr then
        [ ]
      else
        [ "site.network.routerIp must be inside network.homeLanCidr" ]
    )
    ++ (
      if n.address != n.routerIp then [ ] else [ "site.network.address must not equal network.routerIp" ]
    )
    ++ (
      # The appliance exists only on the home LAN; TrueNAS reaches it routed
      # through the gateway. A TrueNAS address inside the home CIDR means the
      # values describe the pre-split network and every firewall pin is wrong.
      if ipInCidr n.truenasIp n.homeLanCidr then
        [ "site.network.truenasIp must be outside network.homeLanCidr (it is routed via the gateway)" ]
      else
        [ ]
    )
    ++ (
      if n.address != broadcastOf n.homeLanCidr then
        [ ]
      else
        [ "site.network.address must not be the broadcast address" ]
    )
    ++ (
      if n.homeBroadcast == broadcastOf n.homeLanCidr then
        [ ]
      else
        [ "site.network.homeBroadcast must be the broadcast of network.homeLanCidr" ]
    )
    ++ (
      if c.airwaveSsdp.airwaveIp == n.truenasIp then
        [ ]
      else
        [ "site.collector.airwaveSsdp.airwaveIp must equal network.truenasIp" ]
    )
    ++ (
      if c.spool.segmentBytes * 2 <= c.spool.maxBytes then
        [ ]
      else
        [ "site.collector.spool.maxBytes must be at least twice spool.segmentBytes" ]
    )
    ++ (
      if site.api.port != c.airwaveSsdp.relayPort then
        [ ]
      else
        [ "site.api.port must differ from collector.airwaveSsdp.relayPort" ]
    )
    ++ (
      if site.api.tlsPort != site.api.port then
        [ ]
      else
        [ "site.api.tlsPort must differ from api.port: the terminator fronts the service" ]
    );

  validateSite =
    site:
    let
      structural = structureErrors site;
    in
    # Report structure first: semantic checks dereference these fields.
    if structural != [ ] then structural else semanticErrors site;

  assertValid =
    site:
    let
      errors = validateSite site;
    in
    if errors == [ ] then
      site
    else
      throw "site validation failed:\n  - ${concatStringsSep "\n  - " errors}";
in
{
  inherit
    isIp
    isCidr
    isMac
    parseCidr
    ipInCidr
    broadcastOf
    validateSite
    assertValid
    ;
}

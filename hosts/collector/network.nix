# Single-homed on the IoT LAN: one interface, renamed by permanent MAC so
# re-plugging or hardware swaps are a one-value change in the configuration
# store. Server-subnet peers (TrueNAS) arrive routed through the VP2440 with
# original source addresses (the gateway does no NAT), so input rules pin
# those flows to the TrueNAS address.
{
  site,
  ...
}:
let
  n = site.network;
  c = site.collector;
in
{
  systemd.network.enable = true;
  networking.useDHCP = false;
  networking.useNetworkd = true;

  # Console recovery tooling (nmtui): NetworkManager manages only wireless
  # interfaces — lan0 and every wired port stay with systemd-networkd, whose
  # static config is the appliance's real network identity. WiFi is for a
  # keyboard-and-monitor session when the wire is unavailable, never the
  # deployed path; the health gate still requires lan0.
  networking.networkmanager = {
    enable = true;
    unmanaged = [
      "interface-name:lan0"
      "interface-name:en*"
      "interface-name:eth*"
    ];
  };

  systemd.network.links."10-${n.interfaceName}" = {
    matchConfig.PermanentMACAddress = n.interfaceMac;
    linkConfig.Name = n.interfaceName;
  };

  systemd.network.networks."20-${n.interfaceName}" = {
    matchConfig.Name = n.interfaceName;
    address = [ "${n.address}/${toString n.prefixLength}" ];
    routes = [ { Gateway = n.routerIp; } ];
    dns = n.dnsServers;
    networkConfig = {
      IPv6AcceptRA = false;
      LinkLocalAddressing = "no";
    };
    # The SSDP relay joins 239.255.255.250 itself; IGMP needs multicast on.
    linkConfig.Multicast = true;
  };

  # Default-drop input policy with exactly the appliance's declared surface.
  # UDP discovery replies cannot ride conntrack (the outbound packet goes to
  # a broadcast or multicast address, the reply comes back unicast), so every
  # reply port the service binds is opened here explicitly.
  networking.nftables.enable = true;
  networking.firewall = {
    enable = true;
    allowPing = true;
    extraInputRules = ''
      # Admin SSH from the trusted admin LAN only.
      ip saddr ${n.adminLanCidr} tcp dport 22 accept comment "collector:ssh-admin"

      # Pull API over TLS (ahara-vpn ADR-0015): the terminator fronts the
      # service at collector.local.ahara.io, so nothing crosses the wire in
      # plaintext.
      ip saddr { ${n.truenasIp}, ${n.adminLanCidr}, ${n.homeLanCidr} } tcp dport ${toString site.api.tlsPort} accept comment "collector:api-tls"

      # The plain port stays reachable until the TrueNAS puller cuts over to
      # the TLS endpoint (docs/backlog.md).
      ip saddr { ${n.truenasIp}, ${n.adminLanCidr}, ${n.homeLanCidr} } tcp dport ${toString site.api.port} accept comment "collector:api"

      # Airwave SSDP ingress (unicast from TrueNAS) and on-link SSDP
      # multicast/broadcast from IoT devices.
      ip saddr { ${n.truenasIp}, ${n.homeLanCidr} } udp dport ${toString c.airwaveSsdp.ssdpPort} accept comment "collector:ssdp"

      # Renderer answers to re-originated M-SEARCH arrive unicast on the
      # relay port.
      ip saddr ${n.homeLanCidr} udp dport ${toString c.airwaveSsdp.relayPort} accept comment "collector:ssdp-replies"

      # Sensor discovery replies: the service binds fixed source ports so
      # these stay narrow (env sensors reply from ${toString c.envSensors.discoveryPort},
      # Kasa devices from ${toString c.kasa.discoveryPort}).
      ip saddr ${n.homeLanCidr} udp dport ${toString c.envSensors.discoveryBindPort} accept comment "collector:env-discovery-replies"
      ip saddr ${n.homeLanCidr} udp dport ${toString c.kasa.discoveryBindPort} accept comment "collector:kasa-discovery-replies"
    '';
  };
}

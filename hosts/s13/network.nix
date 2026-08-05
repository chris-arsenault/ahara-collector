# Single-homed on the home LAN: one interface, renamed by permanent MAC so
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
      # Admin SSH from the home LAN only.
      ip saddr ${n.homeLanCidr} tcp dport 22 accept comment "collector:ssh-home"

      # Pull API: TrueNAS drains readings through the gateway; home-LAN
      # operators reach the same port for health and device listings.
      ip saddr { ${n.truenasIp}, ${n.homeLanCidr} } tcp dport ${toString site.api.port} accept comment "collector:api"

      # Airwave SSDP ingress (unicast from TrueNAS) and on-link SSDP
      # multicast/broadcast from home devices.
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

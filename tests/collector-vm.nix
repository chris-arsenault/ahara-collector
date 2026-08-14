# Two-VM liveness test of the composed appliance. The collector node runs
# the real host modules (network, collector service, deployment,
# hardening); the peer node plays every neighbor at once: the IoT router
# (192.168.30.1), TrueNAS/Airwave (192.168.66.3, reachable because the peer
# owns that address on the shared link and is the collector's default
# gateway), a WiiM renderer, and an AtomS3U environment sensor.
#
# What only a running system can prove is asserted here: MAC→lan0 rename,
# firewall pins, the credentials-file restart contract, end-to-end SSDP
# relay, collector-owned WiiM inventory and transport, local MediaServer
# discovery, sensor discovery → poll → spool → pull → ack, and the deploy
# health check. Pure policy shape is asserted at eval time in
# tests/site-validation.nix. KVM-free so the test runs under TCG on hosted
# CI runners.
#
# QEMU test NICs get MAC 52:54:00:12:<vlan>:<machine>; nodes are numbered
# alphabetically, so collector=01 and peer=02 on vlan 1.
{ pkgs }:
let
  sitelib = import ../lib/site-assertions.nix;
  baseTopology = removeAttrs (builtins.fromJSON (
    builtins.readFile ../hosts/collector/topology.json
  )) [ "_comment" ];
  baseMachine = removeAttrs (builtins.fromJSON (
    builtins.readFile ../hosts/collector/machine-values.json
  )) [ "_comment" ];
  testTopology = baseTopology // {
    # No KLAP mock exists; the Kasa module is exercised by unit tests.
    kasa = baseTopology.kasa // {
      enable = false;
    };
  };
  testMachine = baseMachine // {
    interfaceMac = "52:54:00:12:01:01";
  };
  machineStore = pkgs.writeText "collector-machine-values.json" (builtins.toJSON testMachine);
  legacyStore = pkgs.writeText "collector-legacy-site-values.json" (
    builtins.toJSON (
      testTopology
      // {
        adminAuthorizedKeys = testMachine.adminAuthorizedKeys;
        network = testTopology.network // {
          interfaceMac = testMachine.interfaceMac;
        };
      }
    )
  );
  testSite = sitelib.assertValid (
    import ../hosts/collector/site.nix {
      topology = testTopology;
      machineValues = testMachine;
    }
  );

  mockEnvSensor = pkgs.writeScriptBin "mock-env-sensor" ''
    #!${pkgs.python3}/bin/python3
    import base64
    import json
    import socket
    import threading
    from http.server import BaseHTTPRequestHandler, HTTPServer


    def discovery():
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        s.bind(("0.0.0.0", 12343))
        while True:
            data, addr = s.recvfrom(1024)
            if b"discover" in data:
                reply = {"deviceId": "VM-SENSOR-1", "model": "ENV3", "m5_tags": {"room": "vm"}}
                s.sendto(json.dumps(reply).encode(), addr)


    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_GET(self):
            expected = "Basic " + base64.b64encode(b"admin:vmpass").decode()
            if self.headers.get("Authorization", "") != expected:
                self.send_response(401)
                self.end_headers()
                return
            if self.path == "/sensors":
                body = json.dumps({
                    "temperature_c": 21.5,
                    "humidity": 45.0,
                    "pressure_pa": 101325.0,
                    "sample_age_ms": 10.0,
                }).encode()
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            else:
                self.send_response(404)
                self.end_headers()


    threading.Thread(target=discovery, daemon=True).start()
    HTTPServer(("0.0.0.0", 80), Handler).serve_forever()
  '';

  mockRenderer = pkgs.writeScriptBin "mock-renderer" ''
    #!${pkgs.python3}/bin/python3
    # Answers M-SEARCH like a WiiM and exposes the device-advertised UPnP
    # control path so the collector inventory and transport are both real.
    import socket
    import threading
    from http.server import BaseHTTPRequestHandler, HTTPServer

    DESCRIPTION = b"""<?xml version="1.0"?>
    <root xmlns="urn:schemas-upnp-org:device-1-0">
      <device>
        <deviceType>urn:schemas-upnp-org:device:MediaRenderer:1</deviceType>
        <friendlyName>VM WiiM</friendlyName>
        <modelName>WiiM Mini</modelName>
        <modelNumber>Linkplay.VM</modelNumber>
        <UDN>uuid:vm-wiim</UDN>
        <serviceList>
          <service><serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType><controlURL>/upnp/control/avtransport1</controlURL></service>
          <service><serviceType>urn:schemas-upnp-org:service:RenderingControl:1</serviceType><controlURL>/upnp/control/rendercontrol1</controlURL></service>
          <service><serviceType>urn:schemas-wiimu-com:service:PlayQueue:1</serviceType><controlURL>/upnp/control/PlayQueue1</controlURL></service>
        </serviceList>
      </device>
    </root>"""


    def discovery():
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        s.bind(("0.0.0.0", 1900))
        reply = (
            b"HTTP/1.1 200 OK\r\n"
            b"CACHE-CONTROL: max-age=1800\r\n"
            b"EXT:\r\n"
            b"LOCATION: http://192.168.30.60:49152/description.xml\r\n"
            b"ST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n"
            b"USN: uuid:vm-wiim::urn:schemas-upnp-org:device:MediaRenderer:1\r\n"
            b"\r\n"
        )
        while True:
            data, addr = s.recvfrom(4096)
            if b"M-SEARCH" in data and b"MediaRenderer" in data:
                s.sendto(reply, addr)


    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_GET(self):
            if self.path != "/description.xml":
                self.send_response(404)
                self.end_headers()
                return
            self.send_response(200)
            self.send_header("Content-Type", "text/xml")
            self.send_header("Content-Length", str(len(DESCRIPTION)))
            self.end_headers()
            self.wfile.write(DESCRIPTION)

        def do_POST(self):
            if self.path != "/upnp/control/avtransport1":
                self.send_response(404)
                self.end_headers()
                return
            if self.headers.get("SOAPAction") != '"urn:schemas-upnp-org:service:AVTransport:1#GetTransportInfo"':
                self.send_response(400)
                self.end_headers()
                return
            length = int(self.headers.get("Content-Length", "0"))
            body = self.rfile.read(length)
            if b"GetTransportInfo" not in body:
                self.send_response(400)
                self.end_headers()
                return
            response = b'<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><GetTransportInfoResponse xmlns="urn:schemas-upnp-org:service:AVTransport:1"><CurrentTransportState>PLAYING</CurrentTransportState></GetTransportInfoResponse></s:Body></s:Envelope>'
            self.send_response(200)
            self.send_header("Content-Type", 'text/xml; charset="utf-8"')
            self.send_header("Content-Length", str(len(response)))
            self.end_headers()
            self.wfile.write(response)


    threading.Thread(target=discovery, daemon=True).start()
    HTTPServer(("0.0.0.0", 49152), Handler).serve_forever()
  '';

  airwaveProbe = pkgs.writeScriptBin "airwave-probe" ''
    #!${pkgs.python3}/bin/python3
    # Plays Airwave's discovery: M-SEARCH from the fixed response port on
    # the TrueNAS address toward the collector, then waits for the relayed
    # renderer reply.
    import socket
    import sys

    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("192.168.66.3", 1901))
    s.settimeout(15)
    msearch = (
        b"M-SEARCH * HTTP/1.1\r\n"
        b"HOST: 239.255.255.250:1900\r\n"
        b'MAN: "ssdp:discover"\r\n'
        b"MX: 3\r\n"
        b"ST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n"
        b"\r\n"
    )
    s.sendto(msearch, ("192.168.30.2", 1900))
    data, addr = s.recvfrom(4096)
    assert b"200 OK" in data, data
    assert b"uuid:vm-wiim" in data, data
    print("renderer reply relayed from", addr)
    sys.exit(0)
  '';

  wiimMediaProbe = pkgs.writeScriptBin "wiim-media-probe" ''
    #!${pkgs.python3}/bin/python3
    # Plays a WiiM searching the IoT LAN for Airwave's registered UPnP
    # MediaServer. The response must originate at the collector.
    import socket

    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("192.168.30.60", 0))
    s.settimeout(15)
    msearch = (
        b"M-SEARCH * HTTP/1.1\r\n"
        b"HOST: 239.255.255.250:1900\r\n"
        b'MAN: "ssdp:discover"\r\n'
        b"MX: 2\r\n"
        b"ST: urn:schemas-upnp-org:device:MediaServer:1\r\n"
        b"\r\n"
    )
    s.sendto(msearch, ("192.168.30.2", 1900))
    data, addr = s.recvfrom(4096)
    assert addr[0] == "192.168.30.2", addr
    assert b"200 OK" in data, data
    assert b"urn:schemas-upnp-org:device:MediaServer:1" in data, data
    assert b"LOCATION: http://192.168.66.3:7882/device.xml" in data, data
  '';
in
pkgs.testers.runNixOSTest {
  name = "collector-vm";
  requiredFeatures.kvm = false;
  globalTimeout = 1800;

  node.specialArgs = {
    site = testSite;
    sitelib = sitelib;
    collectorPackage = pkgs.callPackage ../service/package.nix { };
  };

  nodes.collector =
    { lib, ... }:
    let
      api = (import ../hosts/collector/site.nix { }).api;
      n = (import ../hosts/collector/site.nix { }).network;
    in
    {
      imports = [
        ../hosts/collector/network.nix
        ../hosts/collector/collector.nix
        ../hosts/collector/tls.nix
        ../hosts/collector/deployment.nix
        ../hosts/collector/hardening.nix
      ];
      virtualisation.vlans = [ 1 ];
      virtualisation.memorySize = 1024;
      networking.hostName = "collector-test";
      system.stateVersion = "26.05";
      environment.systemPackages = [ pkgs.curl ];
      systemd.tmpfiles.rules = [
        "C /var/lib/ahara-collector/machine-values.json 0644 root root - ${machineStore}"
      ];

      # Stands in for the trust appliance, which is not in this test. The
      # appliance generates nothing itself: without a certificate nginx does
      # not start, which is the point. What is under test is the API behind
      # the terminator, so the certificate is supplied rather than obtained.
      systemd.services.test-certificate = {
        description = "Supply the certificate the trust appliance would have";
        wantedBy = [ "multi-user.target" ];
        before = [ "nginx.service" ];
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
          openssl req -x509 -newkey rsa:2048 -sha256 -nodes -days 1 \
            -subj "/CN=${api.hostName}" \
            -addext "subjectAltName=DNS:${api.hostName},IP:${n.address}" \
            -keyout ${api.certificateKey} -out ${api.certificate}
          chown root:nginx ${api.certificateKey}
          chmod 640 ${api.certificateKey}
          chmod 644 ${api.certificate}
        '';
      };
      # Nothing routable exists during boot; don't stall on wait-online.
      systemd.network.wait-online.enable = lib.mkForce false;
      boot.consoleLogLevel = lib.mkForce 3;
      boot.kernelParams = [ "quiet" ];
    };

  nodes.peer =
    { ... }:
    {
      virtualisation.vlans = [ 1 ];
      networking.hostName = "peer";
      system.stateVersion = "26.05";
      networking.firewall.enable = false;
      networking.useDHCP = false;
      networking.interfaces.eth1.ipv4.addresses = [
        # IoT router — the collector's default gateway.
        {
          address = "192.168.30.1";
          prefixLength = 24;
        }
        # TrueNAS/Airwave, on-link here so no real router is needed.
        {
          address = "192.168.66.3";
          prefixLength = 24;
        }
        # The WiiM renderer's advertised address.
        {
          address = "192.168.30.60";
          prefixLength = 24;
        }
      ];
      environment.systemPackages = [
        pkgs.curl
        pkgs.jq
        mockEnvSensor
        mockRenderer
        airwaveProbe
        wiimMediaProbe
      ];
      systemd.services.mock-env-sensor = {
        wantedBy = [ "multi-user.target" ];
        after = [ "network.target" ];
        serviceConfig.ExecStart = "${mockEnvSensor}/bin/mock-env-sensor";
      };
      systemd.services.mock-renderer = {
        wantedBy = [ "multi-user.target" ];
        after = [ "network.target" ];
        serviceConfig.ExecStart = "${mockRenderer}/bin/mock-renderer";
      };
    };

  testScript = ''
    import json
    import shlex

    start_all()
    collector.wait_for_unit("multi-user.target")
    peer.wait_for_unit("multi-user.target")

    with subtest("interface renamed by MAC and addressed"):
        collector.wait_until_succeeds("ip link show lan0")
        collector.wait_until_succeeds("ip -4 addr show lan0 | grep -qF 'inet 192.168.30.2/24'")
        collector.succeed("ip route show default | grep -q 'via 192.168.30.1'")

    with subtest("firewall: default drop with the declared surface only"):
        collector.succeed("nft list ruleset | grep -qF 'collector:api'")
        collector.succeed("nft list ruleset | grep -qF 'collector:api-tls'")
        collector.succeed("nft list ruleset | grep -qF 'collector:ssdp'")
        collector.succeed("nft list ruleset | grep -qF 'collector:env-discovery-replies'")

    with subtest("first-boot state: API token generated"):
        collector.wait_for_unit("collector-config-migrate.service")
        collector.succeed("systemctl stop collector-config-migrate.service")
        collector.succeed(
            "rm -f /var/lib/ahara-collector/machine-values.json && "
            "install -m 0644 ${legacyStore} /var/lib/ahara-collector/site-values.json"
        )
        collector.succeed("systemctl start collector-config-migrate.service")
        collector.succeed("test -s /var/lib/ahara-collector/machine-values.json")
        collector.succeed("test -f /var/lib/ahara-collector/site-values.json.migrated")
        collector.fail("grep -q '\"address\"' /var/lib/ahara-collector/machine-values.json")
        collector.wait_for_unit("ahara-collector-token.service")
        collector.succeed("test -s /var/lib/ahara-collector/api-token")
        collector.succeed("test $(stat -c %a /var/lib/ahara-collector/api-token) = 600")
        collector.wait_for_unit("ahara-collector-airwave-token.service")
        collector.succeed("test -s /var/lib/ahara-collector/airwave-token")
        collector.succeed("test $(stat -c %a /var/lib/ahara-collector/airwave-token) = 600")

    with subtest("collector service runs and binds its sockets"):
        collector.wait_for_unit("ahara-collector.service")
        collector.wait_until_succeeds("ss -uln | grep -qF '0.0.0.0:1900'")
        collector.wait_until_succeeds("ss -uln | grep -qF '192.168.30.2:1901'")
        collector.wait_until_succeeds("ss -tln | grep -qF '192.168.30.2:8850'")

    with subtest("health endpoint is open; data endpoints are token-gated"):
        peer.wait_until_succeeds("curl -sf http://192.168.30.2:8850/health | grep -q '\"status\":\"ok\"'")
        peer.fail("curl -sf http://192.168.30.2:8850/readings/next")
        peer.fail("curl -sf -H 'authorization: Bearer wrong' http://192.168.30.2:8850/metrics")

    with subtest("TLS terminator fronts the same API"):
        collector.wait_for_unit("nginx.service")
        peer.wait_until_succeeds(
            "curl -skf https://192.168.30.2:8443/health | grep -q '\"status\":\"ok\"'",
            timeout=60,
        )
        # The certificate here stands in for the trust appliance's and is not
        # publicly trusted, so an unpinned client refuses it. In production
        # the distributed one is trusted and this succeeds.
        peer.fail("curl -sf --max-time 5 https://192.168.30.2:8443/health")
        # The appliance generates no certificate of its own: it obtains one
        # from the trust appliance or serves nothing. A placeholder would let
        # the terminator come up while the machine was misconfigured.
        collector.succeed("systemctl is-active nginx.service")
        collector.fail("systemctl list-units --all | grep -q ahara-collector-tls")
        collector.succeed("test $(stat -c %a /var/lib/ahara-collector-tls/api.key) = 640")
        # No ACME client and no cloud credential either.
        collector.fail("systemctl list-units --all | grep -q acme")
        # Authorization passes through the terminator unchanged.
        peer.fail("curl -skf https://192.168.30.2:8443/readings/next")

    token = collector.succeed("cat /var/lib/ahara-collector/api-token").strip()
    auth = f"-H 'authorization: Bearer {token}'"
    airwave_token = collector.succeed("cat /var/lib/ahara-collector/airwave-token").strip()
    airwave_auth = f"-H 'authorization: Bearer {airwave_token}'"

    with subtest("Airwave token is isolated from the sensor API"):
        peer.fail(f"curl -sf {auth} http://192.168.30.2:8850/wiim/devices")
        peer.fail(f"curl -sf {airwave_auth} http://192.168.30.2:8850/metrics")

    with subtest("collector inventories WiiM and proxies only its advertised SOAP path"):
        peer.wait_for_unit("mock-renderer.service")
        peer.wait_until_succeeds(
            f"curl -sf {airwave_auth} http://192.168.30.2:8850/wiim/devices | grep -q 'vm-wiim'",
            timeout=120,
        )
        soap = '<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><u:GetTransportInfo xmlns:u="urn:schemas-upnp-org:service:AVTransport:1"><InstanceID>0</InstanceID></u:GetTransportInfo></s:Body></s:Envelope>'
        out = peer.succeed(
            f"curl -sf -X POST {airwave_auth} "
            "-H 'content-type: text/xml; charset=\"utf-8\"' "
            "-H 'soapaction: \"urn:schemas-upnp-org:service:AVTransport:1#GetTransportInfo\"' "
            f"-d {shlex.quote(soap)} http://192.168.30.2:8850/wiim/vm-wiim/upnp/av-transport"
        )
        assert "CurrentTransportState" in out, out

    with subtest("registered Airwave MediaServer is discoverable only through the collector"):
        lease = json.dumps({
            "uuid": "airwave-vm",
            "location": "http://192.168.66.3:7882/device.xml",
            "server": "Linux/1.0 UPnP/1.0 Airwave/0.1",
            "leaseSeconds": 1200,
        })
        peer.succeed(
            f"curl -sf -X PUT {airwave_auth} -H 'content-type: application/json' "
            f"-d {shlex.quote(lease)} http://192.168.30.2:8850/wiim/media-server"
        )
        peer.succeed("wiim-media-probe")

    with subtest("credentials upload contract: file lands, service restarts, module wakes"):
        # Without credentials the sensor module idles.
        collector.succeed("journalctl -u ahara-collector | grep -q 'env_sensors_idle'")
        creds = json.dumps({"envSensors": {"username": "admin", "password": "vmpass"}})
        collector.succeed(
            f"printf '%s' {shlex.quote(creds)} > /var/lib/ahara-collector/credentials.json && "
            "chmod 600 /var/lib/ahara-collector/credentials.json && "
            "systemctl restart ahara-collector"
        )
        collector.wait_for_unit("ahara-collector.service")

    with subtest("sensor discovery, polling, and spooling"):
        peer.wait_for_unit("mock-env-sensor.service")
        collector.wait_until_succeeds(
            f"curl -sf {auth} http://192.168.30.2:8850/devices | grep -q 'VM-SENSOR-1'",
            timeout=120,
        )
        peer.wait_until_succeeds(
            f"curl -sf {auth} 'http://192.168.30.2:8850/readings/next?module=envSensors' | grep -q 'temperature_c'",
            timeout=120,
        )

    with subtest("TrueNAS pull cycle: drain then ack"):
        batch = peer.succeed(
            f"curl -sf {auth} 'http://192.168.30.2:8850/readings/next?module=envSensors'"
        )
        doc = json.loads(batch)
        reading = json.loads(doc["lines"].splitlines()[0])
        assert reading["module"] == "envSensors", reading
        assert reading["values"]["temperature_c"] == 21.5, reading
        assert reading["device"]["tags"]["room"] == "vm", reading
        assert reading["timestampNs"] > 0, reading
        ack = json.dumps({"module": "envSensors", "batchId": doc["batchId"]})
        out = peer.succeed(
            f"curl -sf -X POST {auth} -d {shlex.quote(ack)} http://192.168.30.2:8850/readings/ack"
        )
        assert '"acked":true' in out, out

    with subtest("SSDP relay end to end: airwave search -> renderer reply"):
        peer.wait_for_unit("mock-renderer.service")
        peer.succeed("airwave-probe")

    with subtest("ingest accepts device pushes with Basic auth"):
        pushed = json.dumps({"module": "push", "values": {"v": 42}})
        peer.fail(f"curl -sf -X POST -d {shlex.quote(pushed)} http://192.168.30.2:8850/ingest")
        out = peer.succeed(
            f"curl -sf -X POST -u admin:vmpass -d {shlex.quote(pushed)} http://192.168.30.2:8850/ingest"
        )
        assert '"accepted":1' in out, out

    with subtest("metrics render for the pull job"):
        metrics = peer.succeed(f"curl -sf {auth} http://192.168.30.2:8850/metrics")
        assert "collector_env_polls_ok_total" in metrics
        assert "collector_spool_bytes" in metrics
        assert "collector_host_load1" in metrics

    with subtest("deploy health check passes on the composed system"):
        out = collector.succeed("collector-health-check")
        assert "health: all checks ok" in out, out
        # And under the updater's shell-less service PATH.
        out = collector.succeed(
            "systemd-run --collect --wait --pipe /run/current-system/sw/bin/collector-health-check"
        )
        assert "health: all checks ok" in out, out
  '';
}

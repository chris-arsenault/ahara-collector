# Two-VM liveness test of the composed appliance. The collector node runs
# the real host modules (network, collector service, deployment,
# hardening); the peer node plays every neighbor at once: the home router
# (192.168.65.1), TrueNAS/Airwave (192.168.66.3, reachable because the peer
# owns that address on the shared link and is the collector's default
# gateway), a WiiM renderer, and an AtomS3U environment sensor.
#
# What only a running system can prove is asserted here: MAC→lan0 rename,
# firewall pins, the credentials-file restart contract, end-to-end SSDP
# relay, sensor discovery → poll → spool → pull → ack, and the deploy
# health check. Pure policy shape is asserted at eval time in
# tests/site-validation.nix. KVM-free so the test runs under TCG on hosted
# CI runners.
#
# QEMU test NICs get MAC 52:54:00:12:<vlan>:<machine>; nodes are numbered
# alphabetically, so collector=01 and peer=02 on vlan 1.
{ pkgs }:
let
  sitelib = import ../lib/site-assertions.nix;
  baseValues =
    builtins.removeAttrs (builtins.fromJSON (builtins.readFile ../hosts/s13/site-values.json))
      [
        "_comment"
      ];
  testValues = baseValues // {
    network = baseValues.network // {
      interfaceMac = "52:54:00:12:01:01";
    };
    # No KLAP mock exists; the Kasa module is exercised by unit tests.
    kasa = baseValues.kasa // {
      enable = false;
    };
  };
  testSite = sitelib.assertValid (import ../hosts/s13/site.nix { values = testValues; });

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
    # Answers relayed M-SEARCH like a WiiM: unicast 200 OK back to the
    # searcher (the collector's relay port).
    import socket

    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("0.0.0.0", 1900))
    reply = (
        b"HTTP/1.1 200 OK\r\n"
        b"CACHE-CONTROL: max-age=1800\r\n"
        b"EXT:\r\n"
        b"LOCATION: http://192.168.65.60:49152/description.xml\r\n"
        b"ST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n"
        b"USN: uuid:vm-wiim::urn:schemas-upnp-org:device:MediaRenderer:1\r\n"
        b"\r\n"
    )
    while True:
        data, addr = s.recvfrom(4096)
        if b"M-SEARCH" in data and b"MediaRenderer" in data:
            s.sendto(reply, addr)
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
    s.sendto(msearch, ("192.168.65.3", 1900))
    data, addr = s.recvfrom(4096)
    assert b"200 OK" in data, data
    assert b"uuid:vm-wiim" in data, data
    print("renderer reply relayed from", addr)
    sys.exit(0)
  '';
in
pkgs.testers.runNixOSTest {
  name = "s13-vm";
  requiredFeatures.kvm = false;
  globalTimeout = 1800;

  node.specialArgs = {
    site = testSite;
    sitelib = sitelib;
    collectorPackage = pkgs.callPackage ../service/package.nix { };
  };

  nodes.collector =
    { lib, ... }:
    {
      imports = [
        ../hosts/s13/network.nix
        ../hosts/s13/collector.nix
        ../hosts/s13/deployment.nix
        ../hosts/s13/hardening.nix
      ];
      virtualisation.vlans = [ 1 ];
      virtualisation.memorySize = 1024;
      networking.hostName = "s13-test";
      system.stateVersion = "26.05";
      environment.systemPackages = [ pkgs.curl ];
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
        # Home router — the collector's default gateway.
        {
          address = "192.168.65.1";
          prefixLength = 24;
        }
        # TrueNAS/Airwave, on-link here so no real router is needed.
        {
          address = "192.168.66.3";
          prefixLength = 24;
        }
        # The WiiM renderer's advertised address.
        {
          address = "192.168.65.60";
          prefixLength = 24;
        }
      ];
      environment.systemPackages = [
        pkgs.curl
        pkgs.jq
        mockEnvSensor
        mockRenderer
        airwaveProbe
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
        collector.wait_until_succeeds("ip -4 addr show lan0 | grep -qF 'inet 192.168.65.3/24'")
        collector.succeed("ip route show default | grep -q 'via 192.168.65.1'")

    with subtest("firewall: default drop with the declared surface only"):
        collector.succeed("nft list ruleset | grep -qF 'collector:api'")
        collector.succeed("nft list ruleset | grep -qF 'collector:ssdp'")
        collector.succeed("nft list ruleset | grep -qF 'collector:env-discovery-replies'")

    with subtest("first-boot state: API token and deploy key generated"):
        collector.wait_for_unit("ahara-collector-token.service")
        collector.succeed("test -s /var/lib/ahara-collector/api-token")
        collector.succeed("test $(stat -c %a /var/lib/ahara-collector/api-token) = 600")
        collector.wait_for_unit("s13-deploy-keygen.service")
        collector.succeed("test -s /var/lib/ahara-collector/deploy-key.pub")

    with subtest("collector service runs and binds its sockets"):
        collector.wait_for_unit("ahara-collector.service")
        collector.wait_until_succeeds("ss -uln | grep -qF '0.0.0.0:1900'")
        collector.wait_until_succeeds("ss -uln | grep -qF '192.168.65.3:1901'")
        collector.wait_until_succeeds("ss -tln | grep -qF '192.168.65.3:8850'")

    with subtest("health endpoint is open; data endpoints are token-gated"):
        peer.wait_until_succeeds("curl -sf http://192.168.65.3:8850/health | grep -q '\"status\":\"ok\"'")
        peer.fail("curl -sf http://192.168.65.3:8850/readings/next")
        peer.fail("curl -sf -H 'authorization: Bearer wrong' http://192.168.65.3:8850/metrics")

    token = collector.succeed("cat /var/lib/ahara-collector/api-token").strip()
    auth = f"-H 'authorization: Bearer {token}'"

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
            f"curl -sf {auth} http://192.168.65.3:8850/devices | grep -q 'VM-SENSOR-1'",
            timeout=120,
        )
        peer.wait_until_succeeds(
            f"curl -sf {auth} http://192.168.65.3:8850/readings/next | grep -q 'environment,'",
            timeout=120,
        )

    with subtest("TrueNAS pull cycle: drain then ack"):
        batch = peer.succeed(f"curl -sf {auth} http://192.168.65.3:8850/readings/next")
        doc = json.loads(batch)
        assert "temperature_c=21.5" in doc["lines"], doc["lines"]
        assert "room=vm" in doc["lines"], doc["lines"]
        ack = json.dumps({"batchId": doc["batchId"]})
        out = peer.succeed(
            f"curl -sf -X POST {auth} -d {shlex.quote(ack)} http://192.168.65.3:8850/readings/ack"
        )
        assert '"acked":true' in out, out

    with subtest("SSDP relay end to end: airwave search -> renderer reply"):
        peer.wait_for_unit("mock-renderer.service")
        peer.succeed("airwave-probe")

    with subtest("ingest accepts device pushes with Basic auth"):
        peer.fail("curl -sf -X POST -d 'm v=1i 1' http://192.168.65.3:8850/ingest")
        out = peer.succeed(
            "curl -sf -X POST -u admin:vmpass -d 'pushed v=42i 7' http://192.168.65.3:8850/ingest"
        )
        assert '"accepted":1' in out, out

    with subtest("metrics render for the pull job"):
        metrics = peer.succeed(f"curl -sf {auth} http://192.168.65.3:8850/metrics")
        assert "collector_env_polls_ok_total" in metrics
        assert "collector_spool_bytes" in metrics
        assert "collector_host_load1" in metrics

    with subtest("deploy health check passes on the composed system"):
        out = collector.succeed("s13-health-check")
        assert "health: all checks ok" in out, out
        # And under the updater's shell-less service PATH.
        out = collector.succeed(
            "systemd-run --collect --wait --pipe /run/current-system/sw/bin/s13-health-check"
        )
        assert "health: all checks ok" in out, out
  '';
}

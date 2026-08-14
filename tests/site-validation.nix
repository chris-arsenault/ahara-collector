# Eval-time contract tests for lib/site-assertions.nix: the committed
# placeholder site must validate, and each deliberately broken variant must
# be rejected. Runs entirely at evaluation; the derivation only records the
# verdict.
{ pkgs }:
let
  lib = import ../lib/site-assertions.nix;
  site = import ../hosts/collector/site.nix { };
  topology = removeAttrs (builtins.fromJSON (builtins.readFile ../hosts/collector/topology.json)) [
    "_comment"
  ];
  machine = removeAttrs (builtins.fromJSON (
    builtins.readFile ../hosts/collector/machine-values.json
  )) [ "_comment" ];
  legacyValues = topology // {
    adminAuthorizedKeys = machine.adminAuthorizedKeys;
    network = topology.network // {
      address = "192.168.65.10";
      interfaceMac = machine.interfaceMac;
    };
  };
  migratedSite = import ../hosts/collector/site.nix { inherit legacyValues; };

  goodErrors = lib.validateSite site;

  withNetwork = overrides: site // { network = site.network // overrides; };

  brokenCases = [
    {
      name = "missing-address";
      s = site // {
        network = builtins.removeAttrs site.network [ "address" ];
      };
    }
    {
      name = "address-off-subnet";
      s = withNetwork { address = "10.0.0.5"; };
    }
    {
      name = "malformed-admin-cidr";
      s = withNetwork { adminLanCidr = "not-a-cidr"; };
    }
    {
      name = "address-is-broadcast";
      s = withNetwork { address = "192.168.30.255"; };
    }
    {
      name = "address-equals-router";
      s = withNetwork { address = site.network.routerIp; };
    }
    {
      name = "truenas-inside-iot-lan";
      s = withNetwork { truenasIp = "192.168.30.9"; };
    }
    {
      name = "malformed-mac";
      s = withNetwork { interfaceMac = "not-a-mac"; };
    }
    {
      name = "broadcast-does-not-match-cidr";
      s = withNetwork { homeBroadcast = "192.168.64.255"; };
    }
    {
      name = "no-admin-keys";
      s = site // {
        host = site.host // {
          adminAuthorizedKeys = [ ];
        };
      };
    }
    {
      name = "privileged-api-port";
      s = site // {
        api = {
          port = 443;
        };
      };
    }
    {
      name = "api-port-collides-with-wiim-ssdp";
      s = site // {
        api = {
          port = site.collector.wiim.ssdpPort;
        };
      };
    }
    {
      name = "media-server-ip-diverges-from-truenas";
      s = site // {
        collector = site.collector // {
          wiim = site.collector.wiim // {
            mediaServerIp = "192.168.66.9";
          };
        };
      };
    }
    {
      name = "wiim-discovery-port-collides-with-ssdp";
      s = site // {
        collector = site.collector // {
          wiim = site.collector.wiim // {
            discoveryBindPort = site.collector.wiim.ssdpPort;
          };
        };
      };
    }
    {
      name = "invalid-wiim-media-server-port";
      s = site // {
        collector = site.collector // {
          wiim = site.collector.wiim // {
            mediaServerPort = 0;
          };
        };
      };
    }
    {
      name = "spool-cap-below-two-segments";
      s = site // {
        collector = site.collector // {
          spool = site.collector.spool // {
            maxBytes = site.collector.spool.segmentBytes;
          };
        };
      };
    }
    {
      name = "zero-poll-interval";
      s = site // {
        collector = site.collector // {
          envSensors = site.collector.envSensors // {
            pollIntervalSeconds = 0;
          };
        };
      };
    }
  ];

  accepted = builtins.filter (case: lib.validateSite case.s == [ ]) brokenCases;
  acceptedNames = map (case: case.name) accepted;
in
if goodErrors != [ ] then
  throw "committed placeholder site failed validation:\n  - ${builtins.concatStringsSep "\n  - " goodErrors}"
else if migratedSite.network.address != topology.network.address then
  throw "legacy site-values topology overrode versioned topology"
else if accepted != [ ] then
  throw "broken cases wrongly accepted: ${builtins.concatStringsSep ", " acceptedNames}"
else
  pkgs.runCommand "site-validation" { } ''
    echo "placeholder site valid; ${toString (builtins.length brokenCases)} broken cases rejected"
    touch $out
  ''

# This machine's identity, and the certificate its API terminates TLS with.
#
# Both come from the machine-identity appliance (the ahara-trust repository):
# an enrolled certificate proves which machine this is, and the same identity
# fetches the shared publicly-trusted certificate. This appliance holds no
# cloud credential and runs no ACME client of its own (ADR-0008).
#
# Everything here degrades quietly. Before the appliance exists, or while this
# machine's id is not declared there, the terminator keeps serving the
# self-signed certificate it generated on first boot, and the TrueNAS puller
# keeps working against it.
{ site, ... }:
let
  api = site.api;
in
{
  ahara.enroll = {
    enable = true;
    authorityUrl = "https://trust.local.ahara.io:8443";
    # Two segments under ahara: the AWS role that may one day back this
    # identity is named and tagged from exactly this pair, and a third segment
    # would produce a name no role can be matched to.
    workloadId = "spiffe://ahara/appliance/collector";

    certificate = {
      enable = true;
      # Written where the terminator already looks, so a fetched certificate
      # replaces the self-signed one without the vhost changing.
      destination = api.certificate;
      keyDestination = api.certificateKey;
      reloadUnits = [ "nginx.service" ];
    };
  };
}

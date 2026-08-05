# Composition root: which concerns this machine has, and nothing else. Every
# address and setting lives in site.nix; each imported module reads `site`
# from specialArgs.
{
  pkgs,
  site,
  ...
}:
{
  imports = [
    ./hardware-configuration.nix
    ./disko.nix
    ./network.nix
    ./collector.nix
    ./deployment.nix
    ./hardening.nix
  ];

  networking.hostName = site.host.name;
  system.stateVersion = site.host.stateVersion;

  nix.settings.experimental-features = [
    "nix-command"
    "flakes"
  ];
  nix.gc = {
    automatic = true;
    dates = "weekly";
    options = "--delete-older-than 30d";
  };

  time.timeZone = "America/New_York";

  # Diagnostics for workshop and SSH sessions; services never rely on these.
  environment.systemPackages = with pkgs; [
    curl
    jq
    tcpdump
    dig
  ];
}

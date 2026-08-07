# Access policy for the appliance itself: keyed SSH for one admin user, no
# passwords, no root login. Which networks may reach SSH is enforced by the
# input firewall in network.nix, not here.
{ site, ... }:
{
  services.openssh = {
    enable = true;
    openFirewall = false;
    settings = {
      PasswordAuthentication = false;
      KbdInteractiveAuthentication = false;
      PermitRootLogin = "no";
      AllowUsers = [ "ops" ];
    };
  };

  users.mutableUsers = false;
  users.users.ops = {
    isNormalUser = true;
    extraGroups = [ "wheel" ];
    openssh.authorizedKeys.keys = site.host.adminAuthorizedKeys;
  };
  # ops has no password (mutableUsers = false, key-only SSH), so sudo cannot
  # prompt for one.
  security.sudo.wheelNeedsPassword = false;

  # Physical console logs straight in as ops: with an unprotected bootloader,
  # physical access already equals control, and a locked-password account
  # would make the attached monitor/keyboard useless for debugging.
  services.getty.autologinUser = "ops";
}

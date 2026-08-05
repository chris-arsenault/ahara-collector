# Wrapper that pins the bootstrap script's runtime inputs and hands it the
# flake, disko, and renderer as store paths. The script itself stays plain
# bash so it can be read and reasoned about without nix.
{
  pkgs,
  self,
  diskoPackage,
}:
pkgs.writeShellApplication {
  name = "bootstrap-s13";
  runtimeInputs = with pkgs; [
    coreutils
    gnugrep
    gnused
    iproute2
    nixos-install-tools
    util-linux
  ];
  text = ''
    export S13_BOOTSTRAP_FLAKE=${self}
    export S13_DISKO=${diskoPackage}/bin/disko
    export S13_RENDER=${./render-site-values.sh}
    exec ${pkgs.bash}/bin/bash ${./bootstrap-s13.sh} "$@"
  '';
}

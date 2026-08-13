# Wrapper that pins the bootstrap script's runtime inputs and hands it the
# flake, disko, and renderer as store paths. The script itself stays plain
# bash so it can be read and reasoned about without nix.
{
  pkgs,
  self,
  diskoPackage,
}:
pkgs.writeShellApplication {
  name = "bootstrap-collector";
  runtimeInputs = with pkgs; [
    coreutils
    gnugrep
    gnused
    iproute2
    nixos-install-tools
    util-linux
  ];
  text = ''
    export COLLECTOR_BOOTSTRAP_FLAKE=${self}
    export COLLECTOR_DISKO=${diskoPackage}/bin/disko
    export COLLECTOR_RENDER_MACHINE=${./render-machine-values.sh}
    exec ${pkgs.bash}/bin/bash ${./bootstrap-collector.sh} "$@"
  '';
}

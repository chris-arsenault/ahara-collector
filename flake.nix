{
  description = "Ahara IoT-LAN collector appliance (Beelink S13)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    disko = {
      # Pinned by rev: the bootstrap installer runs disko from this flake on
      # bare hardware, so the exact behavior must not drift under us.
      url = "github:nix-community/disko/ff8702b4de27f72b4c78573dfb89ec74e36abdf1";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      disko,
    }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      sitelib = import ./lib/site-assertions.nix;
      site = sitelib.assertValid (import ./hosts/collector/site.nix { });
      collectorPackage = pkgs.callPackage ./service/package.nix { };
    in
    {
      formatter.${system} = pkgs.nixfmt-tree;

      nixosConfigurations.collector = nixpkgs.lib.nixosSystem {
        inherit system;
        specialArgs = { inherit site sitelib collectorPackage; };
        modules = [
          disko.nixosModules.disko
          ./hosts/collector/configuration.nix
        ];
      };

      # The install disk is a bootstrap flag, never committed: callers pass
      # --argstr disk /dev/disk/by-id/... (the bootstrap script does this).
      # disko's CLI calls this with extra arguments (flake, lib, ...) beyond
      # the --argstr it is given; the ellipsis is load-bearing.
      diskoConfigurations.collector =
        {
          disk ? "/dev/disk/by-id/REPLACE_WITH_INSTALL_DISK",
          ...
        }:
        import ./hosts/collector/disko.nix { inherit disk; };

      packages.${system} = {
        ahara-collector = collectorPackage;
        bootstrap-collector = import ./scripts/bootstrap-collector.nix {
          inherit pkgs self;
          diskoPackage = disko.packages.${system}.disko;
        };
      };

      apps.${system} = builtins.mapAttrs (name: pkg: {
        type = "app";
        program = pkgs.lib.getExe pkg;
      }) self.packages.${system};

      checks.${system} = {
        ahara-collector = collectorPackage;
        site-validation = import ./tests/site-validation.nix { inherit pkgs; };
        collector-system = self.nixosConfigurations.collector.config.system.build.toplevel;
        collector-vm = import ./tests/collector-vm.nix { inherit pkgs; };
      };
    };
}

{
  description = "Ahara home-LAN IoT collector appliance (Beelink S13)";

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
      site = sitelib.assertValid (import ./hosts/s13/site.nix { });
      collectorPackage = pkgs.callPackage ./service/package.nix { };
    in
    {
      formatter.${system} = pkgs.nixfmt-tree;

      nixosConfigurations.s13 = nixpkgs.lib.nixosSystem {
        inherit system;
        specialArgs = { inherit site sitelib collectorPackage; };
        modules = [
          disko.nixosModules.disko
          ./hosts/s13/configuration.nix
        ];
      };

      # The install disk is a bootstrap flag, never committed: callers pass
      # --argstr disk /dev/disk/by-id/... (the bootstrap script does this).
      # disko's CLI calls this with extra arguments (flake, lib, ...) beyond
      # the --argstr it is given; the ellipsis is load-bearing.
      diskoConfigurations.s13 =
        {
          disk ? "/dev/disk/by-id/REPLACE_WITH_INSTALL_DISK",
          ...
        }:
        import ./hosts/s13/disko.nix { inherit disk; };

      packages.${system} = {
        ahara-collector = collectorPackage;
        bootstrap-s13 = import ./scripts/bootstrap-s13.nix {
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
        s13-system = self.nixosConfigurations.s13.config.system.build.toplevel;
        s13-vm = import ./tests/s13-vm.nix { inherit pkgs; };
      };
    };
}

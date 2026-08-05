.PHONY: ci fmt fmt-check validate-nix test-rust test-vm

ci: fmt-check validate-nix test-rust

validate-nix:
	nix flake check --no-build

test-rust:
	cd service && cargo test

test-vm:
	nix build --no-link -L .#checks.x86_64-linux.s13-vm

fmt:
	nix fmt .

fmt-check:
	nix run .#formatter.x86_64-linux -- --ci .

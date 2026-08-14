{
  rustPlatform,
  lib,
}:
rustPlatform.buildRustPackage {
  pname = "ahara-collector";
  version = "0.1.0";
  src = ./.;
  cargoLock.lockFile = ./Cargo.lock;
  meta = {
    description = "Ahara IoT collector: WiiM discovery, device pollers, bounded spool, pull API";
    mainProgram = "ahara-collector";
    license = lib.licenses.mit;
  };
}

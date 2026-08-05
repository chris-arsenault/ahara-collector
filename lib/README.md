# lib

Pure functions only, `builtins`-only so tests and the flake import them
with zero dependencies. `site-assertions.nix` validates the derived site:
`validateSite` returns a list of human-readable errors, `assertValid`
throws them at evaluation time — before any module sees a value.

- **Local quality gates run on the pinned toolchain** (`lefthook.yml`, `scripts/cargo-pinned.sh`):
  the pre-commit and pre-push hooks and the examples gate route through
  `scripts/cargo-pinned.sh`, which resolves the `rust-toolchain.toml` pin via rustup and refuses
  to run on any other cargo, so a shadowing toolchain cannot lint with the wrong compiler.
  `scripts/setup-dev.sh` reports the cargo the gates resolve and installs the ALSA headers
  `examples/desktop_audio` needs on Linux.

{ pkgs, lib, config, ... }:

{
  # https://devenv.sh/basics/
  env = {
    CARGO_HOME = "${config.env.DEVENV_STATE}/cargo";
    CARGO_TARGET_DIR = "${config.env.DEVENV_STATE}/target";
    RUST_BACKTRACE = "1";
  } // lib.optionalAttrs pkgs.stdenv.isDarwin {
    # Help linker find system libraries on macOS
    LIBRARY_PATH = lib.makeLibraryPath (with pkgs; [ libiconv zlib openssl ]);
  };

  # https://devenv.sh/packages/
  packages = with pkgs; [
    gitMinimal
    just # command runner
    jq # release metadata validation and eval artifact checks
    vhs # deterministic TUI recordings and screenshot checkpoints
    # Native build dependencies for linking
    pkg-config
    openssl
    zlib
    libiconv # Explicit libiconv for reliable macOS linking
    inferno # flamegraph from collapsed stacks (`just perf-profile`)
  ] ++ lib.optionals stdenv.isLinux [
    perf # linux `perf` for `just perf-profile`
  ] ++ lib.optionals stdenv.isDarwin [
    samply
  ];

  # Rust environment.
  #
  # The toolchain version lives in `rust-toolchain.toml` so the contributor
  # shell and CI resolve the same compiler from one file. `channel =
  # "stable"` would instead resolve against whatever rust-overlay revision
  # `devenv.yaml` happens to pin, which drifts from CI's floating `@stable`
  # as the lock ages — this repository sat four releases behind that way.
  languages.rust = {
    enable = true;
    toolchainFile = ./rust-toolchain.toml;
    targets = [ ];
  };

  # Keep the Rust template focused. The Rust toolchain still provides a C
  # compiler/linker wrapper; enable languages.c only for C/Rust hybrid projects.
  languages.c.enable = lib.mkForce false;

  # https://devenv.sh/scripts/
  scripts = {
    test-project.exec = "cargo test --workspace --locked";
    check.exec = "cargo check";
    build.exec = "cargo build";
    run.exec = "cargo run";
    fmt.exec = "cargo fmt";
    lint.exec = "cargo clippy";
  };

  enterTest = "test-project";

  # Keep the generated configuration regular and tracked for linked worktrees.
  files.".pre-commit-config.yaml".copyMode = "copy";

  # https://devenv.sh/tasks/
  tasks = {
    "cargo:check" = {
      exec = ''
        if [ -f "Cargo.toml" ]; then
          cargo check || echo "⚠️  Cargo check failed, but continuing with shell startup..."
        fi
      '';
      # Removed 'before = [ "devenv:enterShell" ]' so compile errors don't block shell startup
    };
  };

  # https://devenv.sh/reference/options/#git-hooks
  git-hooks.package =
    if pkgs ? prek && pkgs.prek.meta ? mainProgram then pkgs.prek else pkgs.pre-commit;
  git-hooks.hooks = {
    # Custom hooks using bare `cargo` from PATH so they work both inside
    # devenv shell (nix toolchain) and outside it (system toolchain / agents).
    # The built-in clippy/rustfmt hooks hardcode nix store paths that fail
    # outside the devenv environment (can't find std for the target).
    cargo-fmt = {
      enable = true;
      name = "cargo-fmt";
      entry = "cargo fmt --all -- --color=always";
      files = "\\.rs$";
      pass_filenames = false;
    };
    cargo-clippy = {
      enable = true;
      name = "cargo-clippy";
      entry = "cargo clippy -- -D warnings";
      files = "\\.rs$";
      pass_filenames = false;
    };
  };

  # Enter shell message
  enterShell = ''
    echo "🦀 Rust development environment activated"
    echo "Available commands:"
    echo "  - just      # list common recipes"
    echo "  - run      # cargo run"
    echo "  - build    # cargo build"
    echo "  - test-project # cargo test --workspace --locked"
    echo "  - check    # cargo check"
    echo "  - fmt      # cargo fmt"
    echo "  - lint     # cargo clippy"
    echo ""
    if [ ! -f "Cargo.toml" ]; then
      echo "💡 Create a new project with: cargo init"
    fi
  '';
}

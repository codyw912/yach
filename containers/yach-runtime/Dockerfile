# yach-runtime: isolated Linux environment for headless yach sessions
# (provider rotation, yacht custom-harness runs). `--full-auto` writes and
# shell commands stay inside the container; only explicitly mounted
# directories are host-visible. Mirrors the yacht per-harness runtime
# image convention (pi-agent-runtime, claude-code-runtime).
#
# Build (from the repo root): just runtime-image
# Run:                        just rotate <fixture-dir> --prompt "..."

# Builder and runtime stages must share a Debian release: a newer-glibc
# builder produces a binary the slim runtime cannot load.
FROM rust:1.94-slim-bookworm AS builder
WORKDIR /src
COPY . .
RUN cargo build --release -p yach

FROM debian:bookworm-slim
# git + ca-certificates: scenario repos and TLS to provider APIs; bash is
# the shell tool's interpreter and ships with bookworm-slim already.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /src/target/release/yach /usr/local/bin/yach
# The task workdir; rotation mounts the disposable fixture here.
WORKDIR /work
# Entrypoint-neutral on purpose: launchers (yacht's container backend,
# `just run-isolated`) pass the full argv including the binary name, and
# yacht's `docker run` appends the command with no entrypoint override.
CMD ["yach", "--help"]

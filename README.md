# yach

Yet Another Coding Harness.

A high-performance Rust-native coding harness, inspired by Pi. Phase 1 uses Pi's existing backend as an adapter to validate the shell architecture. Phase 2 replaces Pi piece by piece with native Rust primitives — providers, session management, tool execution, and a plugin host — so the harness can fully leverage Rust's performance, safety, and ecosystem.

This repo currently tracks the product direction in `PRD-v0.1.md` and is preconfigured with the `rust-magic-linter` standard preset at the workspace level.

## Workspace layout

- `crates/yach-cli`
- `crates/yach-ui`
- `crates/yach-proto`
- `crates/yach-adapter-pi-rpc`
- `crates/yach-adapter-pi-sdk`
- `crates/yach-bench`

`yach-proto` now includes a small v0 seed for handshake negotiation and core client/server events so the UI and adapters can converge on one contract early.

## Linting setup

- Clippy policy lives in `Cargo.toml` under `[workspace.lints.clippy]`.
- Threshold and terminology tuning lives in `clippy.toml`.
- Future crates should opt into the workspace policy with:

```toml
[lints]
workspace = true
```

## Planning and next work

Project planning, roadmap, status links, and the current next-work queue live in `docs/project-os/README.md`.

# yach

Yet Another Coding Harness.

A high-performance Rust-native coding harness, inspired by Pi. The native Rust backend is the default path for current MVP work: yach owns the UI/backend protocol, sessions, provider loop, tool execution, edit boundary, and extension runtime. Pi remains available as an explicit compatibility/reference backend via `--backend pi`, not as the long-term architecture target.

The original product direction is captured in `PRD-v0.1.md`; active project planning and next-work selection start at `docs/project/README.md`. This repo is preconfigured with the `rust-magic-linter` standard preset at the workspace level.

## Workspace layout

- `crates/yach-cli`
- `crates/yach-ui`
- `crates/yach-proto`
- `crates/yach-backend`
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

Active project planning starts at `docs/project/README.md`.

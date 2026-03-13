# yach

Yet Another Coding Harness.

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

## Next steps

- Create the initial Cargo workspace members from the PRD.
- Run `cargo clippy --workspace --all-targets` once crates exist.

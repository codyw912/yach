# Release profile and absolute binary size (2026-11-17)

## Decision and reference

Use `lto = "fat"`, `codegen-units = 1`, and `strip = true` in
`[profile.release]`. This is the profile used by `cargo install yach`, without
asking users to select a custom profile. Of the four configurations measured,
fat LTO produced the smallest binary and leaves ample estimated headroom under
the deterministic performance job's unchanged 35-minute timeout.

The published-path size reference is **17,645,144 bytes**; the workspace release
reference is **17,640,728 bytes** (17.64 decimal MB), both measured on
2026-11-17 from source commit
`451c65dc9cb531a692f8b65ff23b2a8a410b498b`, with the settings above and the default
features of package `yach`. The source still identifies itself as `0.1.0`; this
is the current-main reference selected for the upcoming 0.2.0 release, not a
claim that a 0.2.0 package has already been published. The reference is for an
x86-64 Linux GNU ELF binary, not a universal size promise across platforms,
linkers, dependency updates, or toolchains.

## Method

The pinned compiler in `rust-toolchain.toml` is Rust **1.98.1**. The actual
compiler reported `rustc 1.98.1 (48a229cea 2026-09-01)`, LLVM 22.1.8, host
`x86_64-unknown-linux-gnu`; Cargo reported
`cargo 1.98.1 (797e8a9bc 2026-08-05)`. Builds used the project's development
shell and its native dependencies, the default C linker, no extra Rust flags,
and `CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1`. Heavy commands were serialized
with a file lock. The machine reported an AMD RYZEN AI MAX+ 395 processor,
eight online logical CPUs, six available to this process, and initial load
averages of 0.12, 0.13, and 0.10.

Each row is **one** successful clean build, in table order, with a separate
fresh `/tmp` target directory. No compiled artifacts were shared between rows;
the compiler and Cargo were invoked directly from the pinned toolchain inside
the development shell. The registry source cache was shared. A preliminary
invocation failed before running Cargo because an external timing executable
was absent; a second preliminary invocation was stopped before completion to
ensure no shared compiler cache could affect the measurements. Neither is a
measurement below. Wall time uses the shell's real-time clock around the Cargo
command only, excluding shell activation and lock acquisition:

```sh
cargo build --release --locked -p yach --target-dir "$fresh_target"
stat -c %s "$fresh_target/release/yach"
```

For the experiment only, settings were supplied through
`CARGO_PROFILE_RELEASE_STRIP=true`, `CARGO_PROFILE_RELEASE_LTO=thin|fat`, and
`CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1`; the baseline cleared these overrides.
All builds exited zero and produced the executable that was measured. These
single-run timings estimate build cost, not a repeatability-tested speed
comparison. File size is the exact filesystem length, not resident memory,
compressed download size, or section totals.

| Configuration | Clean build wall time | Binary bytes | `file` status |
|---|---:|---:|---|
| Current release defaults: no LTO, no stripping | 181.27 s | 29,744,472 | not stripped |
| `strip = true` only | 183.21 s | 22,521,152 | stripped |
| Thin LTO, one codegen unit, stripping | 197.73 s | 19,038,240 | stripped |
| Fat LTO, one codegen unit, stripping | 227.31 s | 17,640,728 | stripped |

Fat LTO removes 12,103,744 bytes (40.69%) versus the current default, and another
1,397,512 bytes versus thin LTO. Its observed clean build cost is 46.04 seconds
above baseline. No optimization-level or panic-strategy change was needed.

## CI budget

The `perf-deterministic` job in `.github/workflows/ci.yml` has a 35-minute
limit. It builds the controller with `--release` into `target/release-root`,
then runs `perf ab --build-cmd cargo`. `build_side` builds both shipping `yach`
and `yach-bench` in each side's `target/release-root`; deterministic workloads
do not require the separate `--features bench` shipping build.

The latest 20 workflow runs were queried with:

```sh
gh run list --repo codyw912/yach --workflow ci.yml --limit 20 \
  --json databaseId,conclusion,createdAt
gh run view <id> --repo codyw912/yach --json jobs
```

The job's display name is `Perf (deterministic)`. Five skipped jobs and one
cancelled job were excluded. The 14 successful samples, from November 10–17:

| Run | Job duration |
|---|---:|
| [42757166406](https://github.com/codyw912/yach/actions/runs/42757166406) | 208 s |
| [42759143741](https://github.com/codyw912/yach/actions/runs/42759143741) | 313 s |
| [42831168099](https://github.com/codyw912/yach/actions/runs/42831168099) | 229 s |
| [43902028362](https://github.com/codyw912/yach/actions/runs/43902028362) | 229 s |
| [43973080232](https://github.com/codyw912/yach/actions/runs/43973080232) | 225 s |
| [43998963259](https://github.com/codyw912/yach/actions/runs/43998963259) | 199 s |
| [43999052347](https://github.com/codyw912/yach/actions/runs/43999052347) | 186 s |
| [44004780012](https://github.com/codyw912/yach/actions/runs/44004780012) | 184 s |
| [44097990809](https://github.com/codyw912/yach/actions/runs/44097990809) | 234 s |
| [44105843434](https://github.com/codyw912/yach/actions/runs/44105843434) | 186 s |
| [44121219722](https://github.com/codyw912/yach/actions/runs/44121219722) | 192 s |
| [44121230755](https://github.com/codyw912/yach/actions/runs/44121230755) | 215 s |
| [44121231506](https://github.com/codyw912/yach/actions/runs/44121231506) | 221 s |
| [44126442441](https://github.com/codyw912/yach/actions/runs/44126442441) | 258 s |

Median: 218 seconds; range: 184–313 seconds. For **later PRs, when both sides
use the tuned profile**, add twice the observed clean `yach` increase:
`2 × (227.31 − 181.27) = 92.08 seconds`. This estimates 310.08 seconds (5m10s)
from the median and 405.08 seconds (6m45s) from the slowest observed job, leaving
about 28 minutes below the timeout. On the profile-changing PR only the head
pays the increase; that is not the case used to choose the settings.

This is an estimate, not a measured tuned CI run: runner hardware and cache
state differ, and controller LTO cost is not isolated by the `-p yach` builds.
The workflow also documents roughly 20 minutes for cold fork builds. Adding
the same two-sided increase gives about 21m32s, leaving roughly 13m28s;
that cold baseline is documentation, not a cold run observed in these samples.
No timeout, performance threshold, or regression-check recipe was loosened.

## What the size gate measures

`crates/yach-bench/src/perf/workloads/binary.rs` selects `Bin::Shipping` and
returns `std::fs::metadata(ctx.yach_bin).len()`. In paired comparisons this is
`target/release-root/release/yach`, built with the ordinary release profile,
not the controller or the `bench` feature binary. The unchanged
`binary/size_bytes` threshold is `size_pct = 5.0`. `judge_value` uses the
absolute delta to test whether the budget is exceeded, then checks direction:
only `current > base` is `Regressed`; a large decrease is `Improved`, not a
failure. The absolute reference above complements this relative gate.

Remaining binary-size work—per-profile gating and cumulative drift history—is
separate and is **not implemented here**.

## Profiling and publication compatibility

Routine `just dev cargo test` and `just lint` use the development profile, so
release LTO and stripping do not change their debug information. The release
consumer scan found `just perf-profile` as the path explicitly requesting
release debug information. `strip = true` means stripping symbols and would
undo its existing `CARGO_PROFILE_RELEASE_DEBUG=1` override. Its build now also
sets `CARGO_PROFILE_RELEASE_STRIP=none`; shipping settings stay in the ordinary
release profile.

The override was verified with a release build of both `yach-bench` and `yach`:

```sh
CARGO_PROFILE_RELEASE_DEBUG=1 CARGO_PROFILE_RELEASE_STRIP=none \
  cargo build --release --locked -p yach-bench -p yach --target-dir "$profiling_target"
```

Cargo finished `release` as `[optimized + debuginfo]` in 4m17s, exit zero.
`file` reported **with debug_info, not stripped** for both binaries;
`readelf -S` showed `.debug_info` and `.symtab` in each. The plain shipping
build remained **stripped**. This verifies the recipe's build overrides,
not a full flamegraph capture.

Cargo 1.98.1 does **not** copy a virtual workspace root's profiles into a
published member manifest. Root-only settings would therefore leave
`cargo install yach` unchanged. The shipping package repeats the same
`[profile.release]` in `crates/yach-cli/Cargo.toml`; `just release-check` compares
both parsed TOML tables using the existing `nix-instantiate` TOML parser and
exits 2 on drift. Nix attribute-set equality ignores formatting and key order.

These tables must remain identical: perf A/B measures the workspace-root
profile, while users installing the published package build its member profile.

A one-key mutation of the member's `lto` from `fat` to `thin` produced:

```text
release blocked: [profile.release] differs between Cargo.toml and crates/yach-cli/Cargo.toml; keep the installed and workspace profiles in sync
error: recipe `release-check` failed with exit code 2
```

Restoring `fat` made the exact metadata-and-profile prefix of `release-check`
exit zero and the parsed table comparison return `true`. The full release
preflight was not run as part of this profile-only verification.

The expected local Cargo warning is:

```text
warning: profiles for the non root package will be ignored, specify profiles at the workspace root:
package:   <checkout>/crates/yach-cli/Cargo.toml
workspace: <checkout>/Cargo.toml
```

The workspace root controls local builds; the member table controls the
standalone package. The hk hooks, CI build/test steps, `just test`, and
`just lint` do not promote Cargo manifest warnings to errors. Their
`-- -D warnings` flags apply to Clippy/rustc, not Cargo's manifest diagnostics;
neither `.cargo/config.toml` nor the measured environment sets warning-denying
Rust flags.

`cargo package --no-verify --locked --allow-dirty -p yach` was blocked by the
registry's missing `yach-catalog` package when preparing its lockfile.
Repeating with `--exclude-lockfile` succeeded (20 files on a clean tree, per
`cargo package --list`) and the extracted,
normalized `Cargo.toml` carried the complete chosen `[profile.release]` table.
This tests archive normalization without pretending the registry dependency
boundary is already ready for publication.

## CLI smoke

The chosen build's `--version` exited zero with `yach 0.1.0`. Its `--help`
also exited zero and printed the interactive invocation, command list,
`--version` / `--help`, RPC, and headless-run options. `file` reported the
chosen ELF executable as `stripped`.

## Repository checks

`just fmt-check` exited zero:

```text
cargo fmt --all --check
fmt_check_exit=0
```

`just lint` exited zero with the member-profile warning above still visible:

```text
toolchain: 1.98.1 (rust-toolchain.toml and workflows agree)
cargo clippy --all-targets --all-features -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.94s
lint_exit=0
```

A first filtered performance invocation attempted to pass an isolated base
directory through `just perf`. The recipe supplies `--base main`, so adding
`--base-dir` is rejected by the controller with
`error: use either --base or --base-dir, not both` (exit 1, 184.94 seconds,
including the successful cold controller build). It did not execute a size
comparison and is not passing gate evidence. Running the unmodified recipe
without that override would use its fixed `perf-base` workspace name, already
registered to a different checkout. To avoid changing that checkout, the
comparison uses the same release controller and `--build-cmd cargo`, but
invokes `perf ab` directly with an isolated baseline directory at the measured
source commit. No recipe behavior was changed for this workaround.

The isolated paired run completed in **583.26 seconds (9m43s)**, excluding
the previously built controller, development-shell activation, and waiting
for the build lock. Both sides' shipping binaries and benchmark workers built
into fresh `release-root` target trees. This is the transition comparison
(old baseline versus tuned head), not a two-tuned-sides CI measurement:

```sh
just dev cargo run -p yach-bench --release --locked -- perf ab \
  --base-dir .perf/release-base --build-cmd cargo \
  --filter 'binary/size_bytes' --deterministic \
  --out .perf/results/release-profile-size-ab.json
```

```text
base-mode: worker
id                | class | verdict  | base     | current  | delta% | budget
binary/size_bytes | size  | improved | 29744472 | 17640728 | -40.69 | 5%
perf_isolated_exit=0
```

The controller's measured current size exactly matches the clean-build
reference. No threshold adjustment was required for the decrease.

Using that measured paired run as a second planning bound, add the 46.04-second
single-side increase once more to account for a future tuned base as well as
the tuned head: `583.26 + 46.04 = 629.30 seconds` (10m29s). Adding the separately
observed 184.94-second cold controller invocation gives a conservative local
end-to-end allowance of 814.24 seconds (13m34s), leaving 21m26s under 35 minutes.
This still excludes the full job's other deterministic rows, but includes
cold worker builds on both sides. As above, the observed CI jobs are
184–313 seconds (median 218; all run IDs are listed in the CI table), so the
paired run provides a substantially more conservative cross-check than the
5m10s–6m45s cache-warm extrapolation, not a claim that CI ran in 583 seconds.

## Standalone package build

Building the unmodified extracted package with
`cargo build --release --manifest-path <extracted>/Cargo.toml` exited 101:

```text
error: no matching package named `yach-catalog` found
location searched: crates.io index
required by package `yach v0.1.0 (<extracted>)`
```

A scratch copy of the extracted normalized package kept its release profile
unchanged and added `[patch.crates-io]` entries for the eight `yach-*`
dependencies and the currently vendored `rig-core`, pointing to the measured
source tree. It started with the workspace lockfile; Cargo needed to update
its root-package resolution, so the build used `--offline` without `--locked`.
It completed in 3m43s with exit zero:

```text
Finished `release` profile [optimized] target(s) in 3m 43s
standalone_patched_exit=0
file: ELF 64-bit LSB pie executable, x86-64, dynamically linked, stripped
standalone_size_bytes=17645144
root_size_bytes=17640728
yach 0.1.0
```

The standalone-install-shaped artifact's absolute reference is therefore
**17,645,144 bytes**, with the same date, source commit, target, toolchain,
default features, and release settings as the table. It is **not byte-size
identical** to the workspace artifact: it is 4,416 bytes (0.025%) larger.
`readelf -SW` shows the difference in `.text` and unwind sections, while
`.rodata` has the same length; both executables are stripped. This is an
unisolated build-path effect, not a release-settings difference; the exact
path/crate-identity cause was not separately measured. The size gate reproduces
17,640,728 bytes exactly. This distinction is recorded rather than claiming
that a locally patched archive is a completed crates.io installation.

The normalized archive's release table was also compared as parsed TOML with
the root and returned `true`. Publication dependency readiness remains outside
this profile change; no registry upload or version bump was performed.

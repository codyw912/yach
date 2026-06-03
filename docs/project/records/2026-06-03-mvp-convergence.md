# MVP Convergence

Date: 2026-06-03

## Goal

Yach's near-term goal is a minimal, extensible coding harness that is fast and
usable for real work out of the box. The target experience is Pi-like in
day-to-day utility, but Rust-native in startup, ownership boundaries, protocol,
session model, tool loop, and extension runtime.

## MVP Bar

The native default should support a normal coding session:

- launch `yach tui` quickly and start typing immediately;
- send provider prompts with streaming responses;
- read, search, and list project files through yach-owned tools;
- create and edit text files through yach-owned exact/create edit tools;
- review and approve local mutations without TUI freezes;
- continue multi-round tool work without artificial default round caps;
- persist and resume enough session state for practical dogfooding;
- surface failures in a way the user can recover from;
- keep Pi as an explicit reference backend only.

## Current Priority

The next work should be an MVP dogfood checkpoint and blocker burn-down. Run
native yach through real coding prompts, capture the highest-impact blocker,
and fix that blocker before taking another platform-expansion slice.

## Deferred Until MVP Is Usable

These remain important but should not be the default next move:

- extension templates and developer packaging;
- npm/git extension adapters;
- TypeScript/Rust host packaging ergonomics;
- additional extension lifecycle commands;
- extension-owned mutation, process, network, or broad write tools;
- broad provider settings UI;
- auto-review runtime implementation unless approval friction becomes the top
  dogfood blocker.

## Work Selection Rule

When choosing the next slice, ask whether it makes native yach more usable for a
real coding session this week. If not, defer it unless it blocks that usability
goal directly.

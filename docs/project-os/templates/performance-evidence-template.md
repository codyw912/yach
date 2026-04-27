# Performance Evidence Template

Use this for entries in `../performance-evidence.md` or detailed reports under `docs/benchmarks/`.

```md
## <Measurement title> — YYYY-MM-DD

- **Machine/environment:** <CPU/OS/terminal/relevant env>
- **Command or harness:** <How it was measured>
- **Build/profile mode:** debug | release | other
- **Workload:** <What was replayed or exercised>
- **Result:** <Numbers with units and percentile if relevant>
- **Comparison target:** <Pi, prior yach run, none>
- **Claim supported:** <Exact claim this evidence supports>
- **Confidence/limitations:** <What this does not prove>
- **Artifact/link:** <Report, raw output, trace, screenshot, etc.>
- **Follow-up:** <Next measurement or fix>
```

Do not use performance evidence to imply broad responsiveness wins unless the workload and comparison target support that claim.

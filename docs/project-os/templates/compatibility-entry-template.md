# Compatibility Entry Template

Use this when a compatibility matrix row needs detail beyond `../compatibility.md`.

```md
## <Compatibility area>

- **PRD reference:** <section or link>
- **Category/tier:** Tier A stock RPC | Tier B rich UI | resource compatibility | session compatibility | logic suite | rich UI suite
- **Adapter path:** RPC | SDK sidecar | native later | TBD
- **Implementation status:** planned | in-progress | implemented-unverified | verified | unknown | blocked | deferred
- **Evidence status/link:** <measured/verified/unknown + link>
- **Confidence/limitations:** <What the evidence does and does not prove>
- **Blocker/unknown:** <Named blocker or unknown>
- **Next action:** <Smallest useful follow-up>
```

Example distinction: a surface can be `implemented-unverified` in code while its evidence status remains `unknown` until a smoke test or compatibility suite proves it.

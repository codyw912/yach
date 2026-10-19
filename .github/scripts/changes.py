"""Decide whether a pull request changes anything besides docs.

Reads changed paths, one per line, on stdin and prints `code=true` or
`code=false` for `$GITHUB_OUTPUT`. A path is docs when its first matching
`[[rule]]` in `.sjujperpowers/risk.toml` has tier `low` (today `docs/**`,
`*.md`, `evals/**/*.md`) and it is not protected: verifying-by-risk's
built-in protected paths, the policy's `protected` list, or anything under
`.github/`. The pull request is docs-only when every path is docs. An empty
list counts as code.

CI runs the base revision's copy of this script against the base revision's
`risk.toml`, so a pull request cannot reclassify itself. Globs follow the
risk policy: matched against the full repository-relative path, `*` and `?`
stay within one path segment, `**/` matches zero or more directories, and
`dir/**` matches everything under `dir`. So `*.md` is top-level only.
"""

import re
import sys
import tomllib

# verifying-by-risk's built-ins (hk-conventions.md, "Protected paths"), plus
# everything under .github/: workflow helpers such as this script decide
# what CI runs.
BUILTIN_PROTECTED = (
    "hk.pkl",
    ".config/hk.pkl",
    ".hk/**",
    "nix/hk.nix",
    "scripts/update-hk",
    ".sjujperpowers/risk.toml",
    ".agents/skills/verify-*/SKILL.md",
    ".agents/skills/verify-*/features/**",
    ".agents/skills/verify-*/scripts/**",
    ".claude/skills/verify-*",
    ".claude/skills/verify-*/**",
    ".github/**",
    "devenv.nix",
    "devenv.yaml",
    "devenv.lock",
)


def glob_regex(pattern: str) -> re.Pattern[str]:
    out = []
    i = 0
    while i < len(pattern):
        if pattern.startswith("**/", i):
            out.append("(?:[^/]+/)*")
            i += 3
        elif pattern.startswith("**", i):
            out.append(".*")
            i += 2
        elif pattern[i] == "*":
            out.append("[^/]*")
            i += 1
        elif pattern[i] == "?":
            out.append("[^/]")
            i += 1
        else:
            out.append(re.escape(pattern[i]))
            i += 1
    return re.compile("".join(out))


class Policy:
    def __init__(self, risk: dict) -> None:
        self.default = risk.get("default", "medium")
        self.rules = [
            ([glob_regex(p) for p in rule["paths"]], rule["tier"])
            for rule in risk.get("rule", [])
        ]
        self.protected = [
            glob_regex(p) for p in (*BUILTIN_PROTECTED, *risk.get("protected", []))
        ]

    def tier(self, path: str) -> str:
        for patterns, tier in self.rules:
            if any(p.fullmatch(path) for p in patterns):
                return tier
        return self.default

    def is_docs(self, path: str) -> bool:
        if any(p.fullmatch(path) for p in self.protected):
            return False
        return self.tier(path) == "low"


def is_code(paths: list[str], policy: Policy) -> bool:
    return not paths or not all(policy.is_docs(p) for p in paths)


def main() -> None:
    risk_toml = sys.argv[1] if len(sys.argv) > 1 else ".sjujperpowers/risk.toml"
    with open(risk_toml, "rb") as f:
        policy = Policy(tomllib.load(f))
    paths = [line for line in sys.stdin.read().splitlines() if line]
    print(f"code={'true' if is_code(paths, policy) else 'false'}")


if __name__ == "__main__":
    main()

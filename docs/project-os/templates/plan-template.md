# Plan Template

Use this when creating implementation plans under `docs/plans/`.

```md
---
title: <type: concise title>
type: feat | fix | refactor
status: active
date: YYYY-MM-DD
origin: <optional origin requirements doc>
---

# <Title>

## Overview

<What changes and why.>

## Problem Frame

<What problem this solves; link origin doc if any.>

## Requirements Trace

- R1. <Requirement or success criterion>

## Scope Boundaries

- <Explicit non-goal>

## Context & Research

- <Existing docs/code/patterns to follow>

## Key Technical Decisions

- <Decision>: <Rationale>

## Implementation Units

- U1. **<Unit name>**

**Goal:** <What this unit accomplishes>

**Requirements:** <R IDs or source criteria>

**Dependencies:** <None / U IDs>

**Files:**
- Create: `<path>`
- Modify: `<path>`
- Test: `<path or none -- docs-only>`

**Approach:**
- <Key design notes, not implementation code.>

**Patterns to follow:**
- `<path>`

**Test scenarios:**
- <Specific scenario, or `Test expectation: none -- <reason>`.>

**Verification:**
- <Done signal.>
```

Example: `../../plans/2026-04-26-001-feat-project-os-skeleton-plan.md`.

---
name: autoskill
description: Observe the user's screen via screenpipe, detect repeated research workflows, match them against existing scientific-agent-skills, and draft new skills (or composition recipes that chain existing ones
triggers:
  - autoskill
tools_needed:
  - read
  - write
  - edit
  - bash
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# autoskill

> **Requires a running [screenpipe](https://github.com/screenpipe/screenpipe) daemon.** This skill has no alternate data source — it reads exclusively from the local screenpipe HTTP API (default `http://localhost:3030`). If the daemon isn't running, `run()` raises `ScreenpipeUnreachable` with install instructions.

> **Network access & environment variables.** This skill makes authenticated HTTP requests to (a) the user's local screenpipe daemon on loopback, and (b) the user-configured LLM backend — one of `http://localhost:1234/v1` (LM Studio, default), `https://api.anthropic.com` (opt-in Claude), or a user-supplied BYOK Foundry gateway. The skill reads three environment variables — `SCREENPIPE_TOKEN`, `ANTHROPIC_API_KEY`, `FOUNDRY_API_KEY` — and uses each only to authenticate to the single endpoint its name implies. No other network destinations, no telemetry, no data egress to any third party.

## Overview

Turn the user's own workflow history — captured passively by the local [screenpipe](https://github.com/screenpipe/screenpipe) daemon — into new skills. This skill is on-demand: the user invokes it with a time window, it queries screenpipe's local HTTP API, clusters repeated workflow patterns, compares each pattern against the existing skills in this repo, and produces a staged folder of proposals the user can review, edit, and promote.

## When to Use This Skill

Invoke this skill when the user asks to:
- "Analyze my last 4 hours / day / week and propose new skills."
- "Look at what I've been doing and tell me what's not covered yet."
- "Draft a skill from my recent workflow."
- "Find composition recipes for workflows I repeat."

Do **not** invoke it for one-off questions about screenpipe itself, for real-time screen queries, or without an explicit user request — the skill analyzes sensitive local content and must stay explicitly user-triggered.

## Privacy Posture

- **Screenpipe handles app/window filtering at capture time.** Install a starter deny-list by copying `references/screenpipe-config.yaml` into the user's screenpipe config. Sensitive apps (password managers, messaging, banking) are never OCR'd in the first place.
- **Raw OCR never leaves the machine.** `scripts/fetch_window.py` pulls data over localhost HTTP. `scripts/cluster.py` reduces the timeline to app/duration/title summaries. `scripts/redact.py` strips emails, API keys, bearer tokens, and phone numbers as defense-in-depth before any cluster summary reaches the LLM.
- **LLM backend defaults to `local`.** The recommended setup is [LM Studio](https://lmstudio.ai/) running `Gemma-4-31B-it` — strong reasoning at a size that fits on most workstation GPUs, and no data ever leaves your machine. Cloud backends (`claude`, `foundry`) are opt-in and documented in `config.yaml` for users who explicitly want them. Detection and embeddings always run locally regardless of backend choice.
- **Dry-run mode** (`--plan`) prints the exact timeline that will be analyzed before an

... (truncated from original)
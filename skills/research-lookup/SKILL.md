---
name: research-lookup
description: 'Look up current research information using parallel-cli search (primary, fast web search), the Parallel Chat API (deep research), or Perplexity sonar-pro-search (academic paper searches). Automatical
triggers:
  - search for
  - research lookup
  - find
tools_needed:
  - read
  - write
  - edit
  - bash
version: "0.1.0"
author: K-Dense Inc.
priority: 9
---

# Research Information Lookup

## Overview

This skill provides real-time research information lookup with **intelligent backend routing**:

- **parallel-cli search** (parallel-web skill): **Primary and default backend** for all research queries. Fast, cost-effective web search with academic source prioritization. Uses `parallel-cli search` with `--include-domains` for scholarly sources.
- **Parallel Chat API** (`core` model): Secondary backend for complex, multi-source deep research requiring extended synthesis (60s-5min latency). Use only when explicitly needed.
- **Perplexity sonar-pro-search** (via OpenRouter): Used only for academic-specific paper searches where scholarly database access is critical.

The skill automatically detects query type and routes to the optimal backend.

## When to Use This Skill

Use this skill when you need:

- **Current Research Information**: Latest studies, papers, and findings
- **Literature Verification**: Check facts, statistics, or claims against current research
- **Background Research**: Gather context and supporting evidence for scientific writing
- **Citation Sources**: Find relevant papers and studies to cite
- **Technical Documentation**: Look up specifications, protocols, or methodologies
- **Market/Industry Data**: Current statistics, trends, competitive intelligence
- **Recent Developments**: Emerging trends, breakthroughs, announcements

## Visual Enhancement with Scientific Schematics

**When creating documents with this skill, always consider adding scientific diagrams and schematics to enhance visual communication.**

If your document does not already contain schematics or diagrams:
- Use the **scientific-schematics** skill to generate AI-powered publication-quality diagrams
- Simply describe your desired diagram in natural language

```bash
python scripts/generate_schematic.py "your diagram description" -o figures/output.png
```

---

## Automatic Backend Selection

The skill automatically routes queries to the best backend based on content:

### Routing Logic

```
Query arrives
    |
    +-- Contains academic keywords? (papers, DOI, journal, peer-reviewed, etc.)
    |       YES --> Perplexity sonar-pro-search (academic search mode)
    |
    +-- Needs deep multi-source synthesis? (user says "deep research", "exhaustive")
    |       YES --> Parallel Chat API (core model, 60s-5min)
    |
    +-- Everything else (general research, market data, technical info, analysis)
            --> parallel-cli search (fast, default)
```

### Default: parallel-cli search (parallel-web skill)

**Primary backend for all standard research queries.** Fast, cost-effective, and supports academic source prioritization.

For scientific/technical queries, run two searches to ensure academic coverage:

```bash
# 1. Academic-focused search
parallel-cli search "your research query" -q "keyword1" -q "keyword2" \
  --json --max-results 10 --excerpt-max-chars-total 27000 \
  --include-domains "scholar.google.com,arxiv.org,p

... (truncated from original)
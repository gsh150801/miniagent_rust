---
name: infographics
description: "Create professional infographics using Nano Banana Pro AI with smart iterative refinement. Uses Gemini 3 Pro for quality review. Integrates research-lookup and web search for accurate data. Supports 
triggers:
  - infographics
  - create figure
  - visualize
  - plot data
  - chart
tools_needed:
  - read
  - write
  - edit
  - bash
version: "0.1.0"
priority: 6
---

# Infographics

## Overview

Infographics are visual representations of information, data, or knowledge designed to present complex content quickly and clearly. **This skill uses Nano Banana Pro AI for infographic generation with Gemini 3 Pro quality review and Perplexity Sonar for research.**

**How it works:**
- (Optional) **Research phase**: Gather accurate facts and statistics using Perplexity Sonar
- Describe your infographic in natural language
- Nano Banana Pro generates publication-quality infographics automatically
- **Gemini 3 Pro reviews quality** against document-type thresholds
- **Smart iteration**: Only regenerates if quality is below threshold
- Professional-ready output in minutes
- No design skills required

**Quality Thresholds by Document Type:**
| Document Type | Threshold | Description |
|---------------|-----------|-------------|
| marketing | 8.5/10 | Marketing materials - must be compelling |
| report | 8.0/10 | Business reports - professional quality |
| presentation | 7.5/10 | Slides, talks - clear and engaging |
| social | 7.0/10 | Social media content |
| internal | 7.0/10 | Internal use |
| draft | 6.5/10 | Working drafts |
| default | 7.5/10 | General purpose |

**Simply describe what you want, and Nano Banana Pro creates it.**

## Quick Start

Generate any infographic by simply describing it:

```bash
# Generate a list infographic (default threshold 7.5/10)
python skills/infographics/scripts/generate_infographic.py \
  "5 benefits of regular exercise" \
  -o figures/exercise_benefits.png --type list

# Generate for marketing (highest threshold: 8.5/10)
python skills/infographics/scripts/generate_infographic.py \
  "Product features comparison" \
  -o figures/product_comparison.png --type comparison --doc-type marketing

# Generate with corporate style
python skills/infographics/scripts/generate_infographic.py \
  "Company milestones 2010-2025" \
  -o figures/timeline.png --type timeline --style corporate

# Generate with colorblind-safe palette
python skills/infographics/scripts/generate_infographic.py \
  "Heart disease statistics worldwide" \
  -o figures/health_stats.png --type statistical --palette wong

# Generate WITH RESEARCH for accurate, up-to-date data
python skills/infographics/scripts/generate_infographic.py \
  "Global AI market size and growth projections" \
  -o figures/ai_market.png --type statistical --research
```

**What happens behind the scenes:**
1. **(Optional) Research**: Perplexity Sonar gathers accurate facts, statistics, and data
2. **Generation 1**: Nano Banana Pro creates initial infographic following design best practices
3. **Review 1**: **Gemini 3 Pro** evaluates quality against document-type threshold
4. **Decision**: If quality >= threshold → **DONE** (no more iterations needed!)
5. **If below threshold**: Improved prompt based on critique, regenerate
6. **Repeat**: Until quality meets threshold OR max iterations reached

**Smart Iteration Benefits:**
- ✅ Saves API calls if first g

... (truncated from original)
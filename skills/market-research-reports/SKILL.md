---
name: market-research-reports
description: Generate comprehensive market research reports (50+ pages) in the style of top consulting firms (McKinsey, BCG, Gartner). Features professional LaTeX formatting, extensive visual generation with scien
triggers:
  - market research reports
  - search for
  - find
tools_needed:
  - read
  - write
  - edit
  - bash
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Market Research Reports

## Overview

Market research reports are comprehensive strategic documents that analyze industries, markets, and competitive landscapes to inform business decisions, investment strategies, and strategic planning. This skill generates **professional-grade reports of 50+ pages** with extensive visual content, modeled after deliverables from top consulting firms like McKinsey, BCG, Bain, Gartner, and Forrester.

**Key Features:**
- **Comprehensive length**: Reports are designed to be 50+ pages with no token constraints
- **Visual-rich content**: 5-6 key diagrams generated at start (more added as needed during writing)
- **Data-driven analysis**: Deep integration with research-lookup for market data
- **Multi-framework approach**: Porter's Five Forces, PESTLE, SWOT, BCG Matrix, TAM/SAM/SOM
- **Professional formatting**: Consulting-firm quality typography, colors, and layout
- **Actionable recommendations**: Strategic focus with implementation roadmaps

**Output Format:** LaTeX with professional styling, compiled to PDF. Uses the `market_research.sty` style package for consistent, professional formatting.

## When to Use This Skill

This skill should be used when:
- Creating comprehensive market analysis for investment decisions
- Developing industry reports for strategic planning
- Analyzing competitive landscapes and market dynamics
- Conducting market sizing exercises (TAM/SAM/SOM)
- Evaluating market entry opportunities
- Preparing due diligence materials for M&A activities
- Creating thought leadership content for industry positioning
- Developing go-to-market strategy documentation
- Analyzing regulatory and policy impacts on markets
- Building business cases for new product launches

## Visual Enhancement Requirements

**CRITICAL: Market research reports should include key visual content.**

Every report should generate **6 essential visuals** at the start, with additional visuals added as needed during writing. Start with the most critical visualizations to establish the report framework.

### Visual Generation Tools

**Use `scientific-schematics` for:**
- Market growth trajectory charts
- TAM/SAM/SOM breakdown diagrams (concentric circles)
- Porter's Five Forces diagrams
- Competitive positioning matrices
- Market segmentation charts
- Value chain diagrams
- Technology roadmaps
- Risk heatmaps
- Strategic prioritization matrices
- Implementation timelines/Gantt charts
- SWOT analysis diagrams
- BCG Growth-Share matrices

```bash
# Example: Generate a TAM/SAM/SOM diagram
python skills/scientific-schematics/scripts/generate_schematic.py \
  "TAM SAM SOM concentric circle diagram showing Total Addressable Market $50B outer circle, Serviceable Addressable Market $15B middle circle, Serviceable Obtainable Market $3B inner circle, with labels and arrows pointing to each segment" \
  -o figures/tam_sam_som.png --doc-type report

# Example: Generate Porter's Five Forces
python skills/scientific-schematics/scripts/generate_s

... (truncated from original)
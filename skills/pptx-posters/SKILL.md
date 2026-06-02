---
name: pptx-posters
description: Create research posters using HTML/CSS that can be exported to PDF or PPTX. Use this skill ONLY when the user explicitly requests PowerPoint/PPTX poster format. For standard research posters, use late
triggers:
  - pptx posters
tools_needed:
  - read
  - write
  - edit
  - bash
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# PPTX Research Posters (HTML-Based)

## Overview

**⚠️ USE THIS SKILL ONLY WHEN USER EXPLICITLY REQUESTS PPTX/POWERPOINT POSTER FORMAT.**

For standard research posters, use the **latex-posters** skill instead, which provides better typographic control and is the default for academic conferences.

This skill creates research posters using HTML/CSS, which can then be exported to PDF or converted to PowerPoint format. The web-based approach offers:
- Modern, responsive layouts
- Easy integration of AI-generated visuals
- Quick iteration and preview in browser
- Export to PDF via browser print function
- Conversion to PPTX if specifically needed

## When to Use This Skill

**ONLY use this skill when:**
- User explicitly requests "PPTX poster", "PowerPoint poster", or "PPT poster"
- User specifically asks for HTML-based poster
- User needs to edit poster in PowerPoint after creation
- LaTeX is not available or user requests non-LaTeX solution

**DO NOT use this skill when:**
- User asks for a "poster" without specifying format → Use latex-posters
- User asks for "research poster" or "conference poster" → Use latex-posters
- User mentions LaTeX, tikzposter, beamerposter, or baposter → Use latex-posters

## AI-Powered Visual Element Generation

**STANDARD WORKFLOW: Generate ALL major visual elements using AI before creating the HTML poster.**

This is the recommended approach for creating visually compelling posters:
1. Plan all visual elements needed (hero image, intro, methods, results, conclusions)
2. Generate each element using scientific-schematics or Nano Banana Pro
3. Assemble generated images in the HTML template
4. Add text content around the visuals

**Target: 60-70% of poster area should be AI-generated visuals, 30-40% text.**

---

### CRITICAL: Poster-Size Font Requirements

**⚠️ ALL text within AI-generated visualizations MUST be poster-readable.**

When generating graphics for posters, you MUST include font size specifications in EVERY prompt. Poster graphics are viewed from 4-6 feet away, so text must be LARGE.

**MANDATORY prompt requirements for EVERY poster graphic:**

```
POSTER FORMAT REQUIREMENTS (STRICTLY ENFORCE):
- ABSOLUTE MAXIMUM 3-4 elements per graphic (3 is ideal)
- ABSOLUTE MAXIMUM 10 words total in the entire graphic
- NO complex workflows with 5+ steps (split into 2-3 simple graphics instead)
- NO multi-level nested diagrams (flatten to single level)
- NO case studies with multiple sub-sections (one key point per case)
- ALL text GIANT BOLD (80pt+ for labels, 120pt+ for key numbers)
- High contrast ONLY (dark on white OR white on dark, NO gradients with text)
- MANDATORY 50% white space minimum (half the graphic should be empty)
- Thick lines only (5px+ minimum), large icons (200px+ minimum)
- ONE SINGLE MESSAGE per graphic (not 3 related messages)
```

**⚠️ BEFORE GENERATING: Review your prompt and count elements**
- If your description has 5+ items → STOP. Split into multiple graphics
- If your workflow has 5+ stages

... (truncated from original)
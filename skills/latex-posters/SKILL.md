---
name: latex-posters
description: "Create professional research posters in LaTeX using beamerposter, tikzposter, or baposter. Support for conference presentations, academic posters, and scientific communication. Includes layout design
triggers:
  - latex posters
tools_needed:
  - read
  - write
  - edit
  - bash
version: "0.1.0"
priority: 5
---

# LaTeX Research Posters

## Overview

Research posters are a critical medium for scientific communication at conferences, symposia, and academic events. This skill provides comprehensive guidance for creating professional, visually appealing research posters using LaTeX packages. Generate publication-quality posters with proper layout, typography, color schemes, and visual hierarchy.

## When to Use This Skill

This skill should be used when:
- Creating research posters for conferences, symposia, or poster sessions
- Designing academic posters for university events or thesis defenses
- Preparing visual summaries of research for public engagement
- Converting scientific papers into poster format
- Creating template posters for research groups or departments
- Designing posters that comply with specific conference size requirements (A0, A1, 36×48", etc.)
- Building posters with complex multi-column layouts
- Integrating figures, tables, equations, and citations in poster format

## AI-Powered Visual Element Generation

**STANDARD WORKFLOW: Generate ALL major visual elements using AI before creating the LaTeX poster.**

This is the recommended approach for creating visually compelling posters:
1. Plan all visual elements needed (title, intro, methods, results, conclusions)
2. Generate each element using scientific-schematics or Nano Banana Pro
3. Assemble generated images in the LaTeX template
4. Add text content around the visuals

**Target: 60-70% of poster area should be AI-generated visuals, 30-40% text.**

---

### CRITICAL: Preventing Content Overflow

**⚠️ POSTERS MUST NOT HAVE TEXT OR CONTENT CUT OFF AT EDGES.**

**Common Overflow Problems:**
1. **Title/footer text extending beyond page boundaries**
2. **Too many sections crammed into available space**
3. **Figures placed too close to edges**
4. **Text blocks exceeding column widths**

**Prevention Rules:**

**1. Limit Content Sections (MAXIMUM 5-6 sections for A0):**
```
✅ GOOD - 5 sections with room to breathe:
   - Title/Header
   - Introduction/Problem
   - Methods
   - Results (1-2 key findings)
   - Conclusions

❌ BAD - 8+ sections crammed together:
   - Overview, Introduction, Background, Methods, 
   - Results 1, Results 2, Discussion, Conclusions, Future Work
```

**2. Set Safe Margins in LaTeX:**
```latex
% tikzposter - add generous margins
\documentclass[25pt, a0paper, portrait, margin=25mm]{tikzposter}

% baposter - ensure content doesn't touch edges
\begin{poster}{
  columns=3,
  colspacing=2em,           % Space between columns
  headerheight=0.1\textheight,  % Smaller header
  % Leave space at bottom
}
```

**3. Figure Sizing - Never 100% Width:**
```latex
% Leave margins around figures
\includegraphics[width=0.85\linewidth]{figure.png}  % NOT 1.0\linewidth
```

**4. Check for Overflow Before Printing:**
```bash
# Compile and check PDF at 100% zoom
pdflatex poster.tex

# Look for:
# - Text cut off at any edge
# - Content touching page boundaries  
# - Overfull hbox warnings i

... (truncated from original)
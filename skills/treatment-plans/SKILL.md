---
name: treatment-plans
description: Generate concise (3-4 page), focused medical treatment plans in LaTeX/PDF format for all clinical specialties. Supports general medical treatment, rehabilitation therapy, mental health care, chronic d
triggers:
  - treatment plans
tools_needed:
  - read
  - write
  - edit
  - bash
version: "0.1.0"
author: K-Dense Inc.
priority: 7
---

# Treatment Plan Writing

## Overview

Treatment plan writing is the systematic documentation of clinical care strategies designed to address patient health conditions through evidence-based interventions, measurable goals, and structured follow-up. This skill provides comprehensive LaTeX templates and validation tools for creating **concise, focused** treatment plans (3-4 pages standard) across all medical specialties with full regulatory compliance.

**Critical Principles:**
1. **CONCISE & ACTIONABLE**: Treatment plans default to 3-4 pages maximum, focusing only on clinically essential information that impacts care decisions
2. **Patient-Centered**: Plans must be evidence-based, measurable, and compliant with healthcare regulations (HIPAA, documentation standards)
3. **Minimal Citations**: Use brief in-text citations only when needed to support clinical recommendations; avoid extensive bibliographies

Every treatment plan should include clear goals, specific interventions, defined timelines, monitoring parameters, and expected outcomes that align with patient preferences and current clinical guidelines - all presented as efficiently as possible.

## When to Use This Skill

This skill should be used when:
- Creating individualized treatment plans for patient care
- Documenting therapeutic interventions for chronic disease management
- Developing rehabilitation programs (physical therapy, occupational therapy, cardiac rehab)
- Writing mental health and psychiatric treatment plans
- Planning perioperative and surgical care pathways
- Establishing pain management protocols
- Setting patient-centered goals using SMART criteria
- Coordinating multidisciplinary care across specialties
- Ensuring regulatory compliance in treatment documentation
- Generating professional treatment plans for medical records

## Visual Enhancement with Scientific Schematics

**⚠️ MANDATORY: Every treatment plan MUST include at least 1 AI-generated figure using the scientific-schematics skill.**

This is not optional. Treatment plans benefit greatly from visual elements. Before finalizing any document:
1. Generate at minimum ONE schematic or diagram (e.g., treatment pathway flowchart, care coordination diagram, or therapy timeline)
2. For complex plans: include decision algorithm flowchart
3. For rehabilitation plans: include milestone progression diagram

**How to generate figures:**
- Use the **scientific-schematics** skill to generate AI-powered publication-quality diagrams
- Simply describe your desired diagram in natural language
- Nano Banana Pro will automatically generate, review, and refine the schematic

**How to generate schematics:**
```bash
python scripts/generate_schematic.py "your diagram description" -o figures/output.png
```

The AI will automatically:
- Create publication-quality images with proper formatting
- Review and refine through multiple iterations
- Ensure accessibility (colorblind-friendly, high contrast)
- Save outputs in the figures/ directory

... (truncated from original)
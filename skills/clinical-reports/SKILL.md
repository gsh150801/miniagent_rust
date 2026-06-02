---
name: clinical-reports
description: Write comprehensive clinical reports including case reports (CARE guidelines), diagnostic reports (radiology/pathology/lab), clinical trial reports (ICH-E3, SAE, CSR), and patient documentation (SOAP,
triggers:
  - clinical reports
tools_needed:
  - read
  - write
  - edit
  - bash
version: "0.1.0"
author: K-Dense Inc.
priority: 7
---

# Clinical Report Writing

## Overview

Clinical report writing is the process of documenting medical information with precision, accuracy, and compliance with regulatory standards. This skill covers four major categories of clinical reports: case reports for journal publication, diagnostic reports for clinical practice, clinical trial reports for regulatory submission, and patient documentation for medical records. Apply this skill for healthcare documentation, research dissemination, and regulatory compliance.

**Critical Principle: Clinical reports must be accurate, complete, objective, and compliant with applicable regulations (HIPAA, FDA, ICH-GCP).** Patient privacy and data integrity are paramount. All clinical documentation must support evidence-based decision-making and meet professional standards.

## When to Use This Skill

This skill should be used when:
- Writing clinical case reports for journal submission (CARE guidelines)
- Creating diagnostic reports (radiology, pathology, laboratory)
- Documenting clinical trial data and adverse events
- Preparing clinical study reports (CSR) for regulatory submission
- Writing patient progress notes, SOAP notes, and clinical summaries
- Drafting discharge summaries, H&P documents, or consultation notes
- Ensuring HIPAA compliance and proper de-identification
- Validating clinical documentation for completeness and accuracy
- Preparing serious adverse event (SAE) reports
- Creating data safety monitoring board (DSMB) reports

## Visual Enhancement with Scientific Schematics

**⚠️ MANDATORY: Every clinical report MUST include at least 1 AI-generated figure using the scientific-schematics skill.**

This is not optional. Clinical reports benefit greatly from visual elements. Before finalizing any document:
1. Generate at minimum ONE schematic or diagram (e.g., patient timeline, diagnostic algorithm, or treatment workflow)
2. For case reports: include clinical progression timeline
3. For trial reports: include CONSORT flow diagram

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

**When to add schematics:**
- Patient case timelines and clinical progression diagrams
- Diagnostic algorithm flowcharts
- Treatment protocol workflows
- Anatomical diagrams for case reports
- Clinical trial participant flow diagrams (CONSORT)
- Adverse event classification trees
- Any complex concept that benefits from visualization

For detailed guidanc

... (truncated from original)
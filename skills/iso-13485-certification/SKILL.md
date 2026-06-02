---
name: iso-13485-certification
description: Comprehensive toolkit for preparing ISO 13485 certification documentation for medical device Quality Management Systems. Use when users need help with ISO 13485 QMS documentation, including (1) conduc
triggers:
  - iso 13485 certification
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# ISO 13485 Certification Documentation Assistant

## Overview

This skill helps medical device manufacturers prepare comprehensive documentation for ISO 13485:2016 certification. It provides tools, templates, references, and guidance to create, review, and gap-analyze all required Quality Management System (QMS) documentation.

**What this skill provides:**
- Gap analysis of existing documentation
- Templates for all mandatory documents
- Comprehensive requirements guidance
- Step-by-step documentation creation
- Identification of missing documentation
- Compliance checklists

**When to use this skill:**
- Starting ISO 13485 certification process
- Conducting gap analysis against ISO 13485
- Creating or updating QMS documentation
- Preparing for certification audit
- Transitioning from FDA QSR to QMSR
- Harmonizing with EU MDR requirements

## Core Workflow

### 1. Assess Current State (Gap Analysis)

**When to start here:** User has existing documentation and needs to identify gaps

**Process:**

1. **Collect existing documentation:**
   - Ask user to provide directory of current QMS documents
   - Documents can be in any format (.txt, .md, .doc, .docx, .pdf)
   - Include any procedures, manuals, work instructions, forms

2. **Run gap analysis script:**
   ```bash
   python scripts/gap_analyzer.py --docs-dir <path_to_docs> --output gap-report.json
   ```

3. **Review results:**
   - Identify which of the 31 required procedures are present
   - Identify missing key documents (Quality Manual, MDF, etc.)
   - Calculate compliance percentage
   - Prioritize missing documentation

4. **Present findings to user:**
   - Summarize what exists
   - Clearly list what's missing
   - Provide prioritized action plan
   - Estimate effort required

**Output:** Comprehensive gap analysis report with prioritized action items

### 2. Understand Requirements (Reference Consultation)

**When to use:** User needs to understand specific ISO 13485 requirements

**Available references:**
- `references/iso-13485-requirements.md` - Complete clause-by-clause breakdown
- `references/mandatory-documents.md` - All 31 required procedures explained
- `references/gap-analysis-checklist.md` - Detailed compliance checklist
- `references/quality-manual-guide.md` - How to create Quality Manual

**How to use:**

1. **For specific clause questions:**
   - Read relevant section from `iso-13485-requirements.md`
   - Explain requirements in plain language
   - Provide practical examples

2. **For document requirements:**
   - Consult `mandatory-documents.md`
   - Explain what must be documented
   - Clarify when documents are applicable vs. excludable

3. **For implementation guidance:**
   - Use `quality-manual-guide.md` for policy-level documents
   - Provide step-by-step creation process
   - Show examples of good vs. poor implementation

**Key reference sections to know:**

- **Clause 4:** QMS requirements, documentation, risk management, software validation
- **Clause 5:** Managem

... (truncated from original)
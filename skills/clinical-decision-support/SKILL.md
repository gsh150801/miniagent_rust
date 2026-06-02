---
name: clinical-decision-support
description: Generate professional clinical decision support (CDS) documents for pharmaceutical and clinical research settings, including patient cohort analyses (biomarker-stratified with outcomes) and treatment 
triggers:
  - clinical decision support
tools_needed:
  - read
  - write
  - edit
  - bash
version: "0.1.0"
author: K-Dense Inc.
priority: 7
---

# Clinical Decision Support Documents

## Description

Generate professional clinical decision support (CDS) documents for pharmaceutical companies, clinical researchers, and medical decision-makers. This skill specializes in analytical, evidence-based documents that inform treatment strategies and drug development:

1. **Patient Cohort Analysis** - Biomarker-stratified group analyses with statistical outcome comparisons
2. **Treatment Recommendation Reports** - Evidence-based clinical guidelines with GRADE grading and decision algorithms

All documents are generated as publication-ready LaTeX/PDF files optimized for pharmaceutical research, regulatory submissions, and clinical guideline development.

**Note:** For individual patient treatment plans at the bedside, use the `treatment-plans` skill instead. This skill focuses on group-level analyses and evidence synthesis for pharmaceutical/research settings.

**Writing Style:** For publication-ready documents targeting medical journals, consult the **venue-templates** skill's `medical_journal_styles.md` for guidance on structured abstracts, evidence language, and CONSORT/STROBE compliance.

## Capabilities

### Document Types

**Patient Cohort Analysis**
- Biomarker-based patient stratification (molecular subtypes, gene expression, IHC)
- Molecular subtype classification (e.g., GBM mesenchymal-immune-active vs proneural, breast cancer subtypes)
- Outcome metrics with statistical analysis (OS, PFS, ORR, DOR, DCR)
- Statistical comparisons between subgroups (hazard ratios, p-values, 95% CI)
- Survival analysis with Kaplan-Meier curves and log-rank tests
- Efficacy tables and waterfall plots
- Comparative effectiveness analyses
- Pharmaceutical cohort reporting (trial subgroups, real-world evidence)

**Treatment Recommendation Reports**
- Evidence-based treatment guidelines for specific disease states
- Strength of recommendation grading (GRADE system: 1A, 1B, 2A, 2B, 2C)
- Quality of evidence assessment (high, moderate, low, very low)
- Treatment algorithm flowcharts with TikZ diagrams
- Line-of-therapy sequencing based on biomarkers
- Decision pathways with clinical and molecular criteria
- Pharmaceutical strategy documents
- Clinical guideline development for medical societies

### Clinical Features

- **Biomarker Integration**: Genomic alterations (mutations, CNV, fusions), gene expression signatures, IHC markers, PD-L1 scoring
- **Statistical Analysis**: Hazard ratios, p-values, confidence intervals, survival curves, Cox regression, log-rank tests
- **Evidence Grading**: GRADE system (1A/1B/2A/2B/2C), Oxford CEBM levels, quality of evidence assessment
- **Clinical Terminology**: SNOMED-CT, LOINC, proper medical nomenclature, trial nomenclature
- **Regulatory Compliance**: HIPAA de-identification, confidentiality headers, ICH-GCP alignment
- **Professional Formatting**: Compact 0.5in margins, color-coded recommendations, publication-ready, suitable for regulatory submissions

## Pharmaceutical an

... (truncated from original)
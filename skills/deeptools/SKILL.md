---
name: deeptools
description: NGS analysis toolkit. BAM to bigWig conversion, QC (correlation, PCA, fingerprints), heatmaps/profiles (TSS, peaks), for ChIP-seq, RNA-seq, ATAC-seq visualization.
triggers:
  - deeptools
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# deepTools: NGS Data Analysis Toolkit

## Overview

deepTools is a comprehensive suite of Python command-line tools designed for processing and analyzing high-throughput sequencing data. Use deepTools to perform quality control, normalize data, compare samples, and generate publication-quality visualizations for ChIP-seq, RNA-seq, ATAC-seq, MNase-seq, and other NGS experiments.

**Core capabilities:**
- Convert BAM alignments to normalized coverage tracks (bigWig/bedGraph)
- Quality control assessment (fingerprint, correlation, coverage)
- Sample comparison and correlation analysis
- Heatmap and profile plot generation around genomic features
- Enrichment analysis and peak region visualization

## When to Use This Skill

This skill should be used when:

- **File conversion**: "Convert BAM to bigWig", "generate coverage tracks", "normalize ChIP-seq data"
- **Quality control**: "check ChIP quality", "compare replicates", "assess sequencing depth", "QC analysis"
- **Visualization**: "create heatmap around TSS", "plot ChIP signal", "visualize enrichment", "generate profile plot"
- **Sample comparison**: "compare treatment vs control", "correlate samples", "PCA analysis"
- **Analysis workflows**: "analyze ChIP-seq data", "RNA-seq coverage", "ATAC-seq analysis", "complete workflow"
- **Working with specific file types**: BAM files, bigWig files, BED region files in genomics context

## Quick Start

For users new to deepTools, start with file validation and common workflows:

### 1. Validate Input Files

Before running any analysis, validate BAM, bigWig, and BED files using the validation script:

```bash
python scripts/validate_files.py --bam sample1.bam sample2.bam --bed regions.bed
```

This checks file existence, BAM indices, and format correctness.

### 2. Generate Workflow Template

For standard analyses, use the workflow generator to create customized scripts:

```bash
# List available workflows
python scripts/workflow_generator.py --list

# Generate ChIP-seq QC workflow
python scripts/workflow_generator.py chipseq_qc -o qc_workflow.sh \
    --input-bam Input.bam --chip-bams "ChIP1.bam ChIP2.bam" \
    --genome-size 2913022398

# Make executable and run
chmod +x qc_workflow.sh
./qc_workflow.sh
```

### 3. Most Common Operations

See `assets/quick_reference.md` for frequently used commands and parameters.

## Installation

```bash
uv pip install deeptools
```

## Core Workflows

deepTools workflows typically follow this pattern: **QC → Normalization → Comparison/Visualization**

### ChIP-seq Quality Control Workflow

When users request ChIP-seq QC or quality assessment:

1. **Generate workflow script** using `scripts/workflow_generator.py chipseq_qc`
2. **Key QC steps**:
   - Sample correlation (multiBamSummary + plotCorrelation)
   - PCA analysis (plotPCA)
   - Coverage assessment (plotCoverage)
   - Fragment size validation (bamPEFragmentSize)
   - ChIP enrichment strength (plotFingerprint)

**Interpreting results:**
- **Correlation**: Replicat

... (truncated from original)
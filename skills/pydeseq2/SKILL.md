---
name: pydeseq2
description: Differential gene expression analysis (Python DESeq2). Identify DE genes from bulk RNA-seq counts, Wald tests, FDR correction, volcano/MA plots, for RNA-seq analysis.
triggers:
  - pydeseq2
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# PyDESeq2

## Overview

PyDESeq2 is a Python implementation of DESeq2 for differential expression analysis with bulk RNA-seq data. Design and execute complete workflows from data loading through result interpretation, including single-factor and multi-factor designs, Wald tests with multiple testing correction, optional apeGLM shrinkage, and integration with pandas and AnnData.

## When to Use This Skill

This skill should be used when:
- Analyzing bulk RNA-seq count data for differential expression
- Comparing gene expression between experimental conditions (e.g., treated vs control)
- Performing multi-factor designs accounting for batch effects or covariates
- Converting R-based DESeq2 workflows to Python
- Integrating differential expression analysis into Python-based pipelines
- Users mention "DESeq2", "differential expression", "RNA-seq analysis", or "PyDESeq2"

## Quick Start Workflow

For users who want to perform a standard differential expression analysis:

```python
import pandas as pd
from pydeseq2.dds import DeseqDataSet
from pydeseq2.ds import DeseqStats

# 1. Load data
counts_df = pd.read_csv("counts.csv", index_col=0).T  # Transpose to samples × genes
metadata = pd.read_csv("metadata.csv", index_col=0)

# 2. Filter low-count genes
genes_to_keep = counts_df.columns[counts_df.sum(axis=0) >= 10]
counts_df = counts_df[genes_to_keep]

# 3. Initialize and fit DESeq2
dds = DeseqDataSet(
    counts=counts_df,
    metadata=metadata,
    design="~condition",
    refit_cooks=True
)
dds.deseq2()

# 4. Perform statistical testing
ds = DeseqStats(dds, contrast=["condition", "treated", "control"])
ds.summary()

# 5. Access results
results = ds.results_df
significant = results[results.padj < 0.05]
print(f"Found {len(significant)} significant genes")
```

## Core Workflow Steps

### Step 1: Data Preparation

**Input requirements:**
- **Count matrix:** Samples × genes DataFrame with non-negative integer read counts
- **Metadata:** Samples × variables DataFrame with experimental factors

**Common data loading patterns:**

```python
# From CSV (typical format: genes × samples, needs transpose)
counts_df = pd.read_csv("counts.csv", index_col=0).T
metadata = pd.read_csv("metadata.csv", index_col=0)

# From TSV
counts_df = pd.read_csv("counts.tsv", sep="\t", index_col=0).T

# From AnnData
import anndata as ad
adata = ad.read_h5ad("data.h5ad")
counts_df = pd.DataFrame(adata.X, index=adata.obs_names, columns=adata.var_names)
metadata = adata.obs
```

**Data filtering:**

```python
# Remove low-count genes
genes_to_keep = counts_df.columns[counts_df.sum(axis=0) >= 10]
counts_df = counts_df[genes_to_keep]

# Remove samples with missing metadata
samples_to_keep = ~metadata.condition.isna()
counts_df = counts_df.loc[samples_to_keep]
metadata = metadata.loc[samples_to_keep]
```

### Step 2: Design Specification

The design formula specifies how gene expression is modeled.

**Single-factor designs:**
```python
design = "~condition"  # Simple two-group compariso

... (truncated from original)
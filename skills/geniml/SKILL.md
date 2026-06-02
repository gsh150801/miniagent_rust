---
name: geniml
description: This skill should be used when working with genomic interval data (BED files) for machine learning tasks. Use for training region embeddings (Region2Vec, BEDspace), single-cell ATAC-seq analysis (scEm
triggers:
  - machine learning
  - geniml
  - train model
  - predict
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Geniml: Genomic Interval Machine Learning

## Overview

Geniml is a Python package for building machine learning models on genomic interval data from BED files. It provides unsupervised methods for learning embeddings of genomic regions, single cells, and metadata labels, enabling similarity searches, clustering, and downstream ML tasks.

## Installation

Install geniml using uv:

```bash
uv pip install geniml
```

For ML dependencies (PyTorch, etc.):

```bash
uv pip install 'geniml[ml]'
```

Development version from GitHub:

```bash
uv pip install git+https://github.com/databio/geniml.git
```

## Core Capabilities

Geniml provides five primary capabilities, each detailed in dedicated reference files:

### 1. Region2Vec: Genomic Region Embeddings

Train unsupervised embeddings of genomic regions using word2vec-style learning.

**Use for:** Dimensionality reduction of BED files, region similarity analysis, feature vectors for downstream ML.

**Workflow:**
1. Tokenize BED files using a universe reference
2. Train Region2Vec model on tokens
3. Generate embeddings for regions

**Reference:** See `references/region2vec.md` for detailed workflow, parameters, and examples.

### 2. BEDspace: Joint Region and Metadata Embeddings

Train shared embeddings for region sets and metadata labels using StarSpace.

**Use for:** Metadata-aware searches, cross-modal queries (region→label or label→region), joint analysis of genomic content and experimental conditions.

**Workflow:**
1. Preprocess regions and metadata
2. Train BEDspace model
3. Compute distances
4. Query across regions and labels

**Reference:** See `references/bedspace.md` for detailed workflow, search types, and examples.

### 3. scEmbed: Single-Cell Chromatin Accessibility Embeddings

Train Region2Vec models on single-cell ATAC-seq data for cell-level embeddings.

**Use for:** scATAC-seq clustering, cell-type annotation, dimensionality reduction of single cells, integration with scanpy workflows.

**Workflow:**
1. Prepare AnnData with peak coordinates
2. Pre-tokenize cells
3. Train scEmbed model
4. Generate cell embeddings
5. Cluster and visualize with scanpy

**Reference:** See `references/scembed.md` for detailed workflow, parameters, and examples.

### 4. Consensus Peaks: Universe Building

Build reference peak sets (universes) from BED file collections using multiple statistical methods.

**Use for:** Creating tokenization references, standardizing regions across datasets, defining consensus features with statistical rigor.

**Workflow:**
1. Combine BED files
2. Generate coverage tracks
3. Build universe using CC, CCF, ML, or HMM method

**Methods:**
- **CC (Coverage Cutoff)**: Simple threshold-based
- **CCF (Coverage Cutoff Flexible)**: Confidence intervals for boundaries
- **ML (Maximum Likelihood)**: Probabilistic modeling of positions
- **HMM (Hidden Markov Model)**: Complex state modeling

**Reference:** See `references/consensus_peaks.md` for method comparison, parameters, and examples.

... (truncated from original)
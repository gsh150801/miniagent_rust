---
name: scvi-tools
description: Deep generative models for single-cell omics. Use when you need probabilistic batch correction (scVI), transfer learning, differential expression with uncertainty, or multi-modal integration (TOTALVI,
triggers:
  - scvi tools
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# scvi-tools

## Overview

scvi-tools is a comprehensive Python framework for probabilistic models in single-cell genomics. Built on PyTorch and PyTorch Lightning, it provides deep generative models using variational inference for analyzing diverse single-cell data modalities.

## When to Use This Skill

Use this skill when:
- Analyzing single-cell RNA-seq data (dimensionality reduction, batch correction, integration)
- Working with single-cell ATAC-seq or chromatin accessibility data
- Integrating multimodal data (CITE-seq, multiome, paired/unpaired datasets)
- Analyzing spatial transcriptomics data (deconvolution, spatial mapping)
- Performing differential expression analysis on single-cell data
- Conducting cell type annotation or transfer learning tasks
- Working with specialized single-cell modalities (methylation, cytometry, RNA velocity)
- Building custom probabilistic models for single-cell analysis

## Core Capabilities

scvi-tools provides models organized by data modality:

### 1. Single-Cell RNA-seq Analysis
Core models for expression analysis, batch correction, and integration. See `references/models-scrna-seq.md` for:
- **scVI**: Unsupervised dimensionality reduction and batch correction
- **scANVI**: Semi-supervised cell type annotation and integration
- **AUTOZI**: Zero-inflation detection and modeling
- **VeloVI**: RNA velocity analysis
- **contrastiveVI**: Perturbation effect isolation

### 2. Chromatin Accessibility (ATAC-seq)
Models for analyzing single-cell chromatin data. See `references/models-atac-seq.md` for:
- **PeakVI**: Peak-based ATAC-seq analysis and integration
- **PoissonVI**: Quantitative fragment count modeling
- **scBasset**: Deep learning approach with motif analysis

### 3. Multimodal & Multi-omics Integration
Joint analysis of multiple data types. See `references/models-multimodal.md` for:
- **totalVI**: CITE-seq protein and RNA joint modeling
- **MultiVI**: Paired and unpaired multi-omic integration
- **MrVI**: Multi-resolution cross-sample analysis

### 4. Spatial Transcriptomics
Spatially-resolved transcriptomics analysis. See `references/models-spatial.md` for:
- **DestVI**: Multi-resolution spatial deconvolution
- **Stereoscope**: Cell type deconvolution
- **Tangram**: Spatial mapping and integration
- **scVIVA**: Cell-environment relationship analysis

### 5. Specialized Modalities
Additional specialized analysis tools. See `references/models-specialized.md` for:
- **MethylVI/MethylANVI**: Single-cell methylation analysis
- **CytoVI**: Flow/mass cytometry batch correction
- **Solo**: Doublet detection
- **CellAssign**: Marker-based cell type annotation

## Typical Workflow

All scvi-tools models follow a consistent API pattern:

```python
# 1. Load and preprocess data (AnnData format)
import scvi
import scanpy as sc

adata = scvi.data.heart_cell_atlas_subsampled()
sc.pp.filter_genes(adata, min_counts=3)
sc.pp.highly_variable_genes(adata, n_top_genes=1200)

# 2. Register data with model (specify layers, 

... (truncated from original)
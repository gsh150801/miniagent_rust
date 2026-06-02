---
name: anndata
description: Data structure for annotated matrices in single-cell analysis. Use when working with .h5ad files or integrating with the scverse ecosystem. This is the data format skill—for analysis workflows use sca
triggers:
  - anndata
  - data analysis
  - analyze data
  - statistical test
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# AnnData

## Overview

AnnData is a Python package for handling annotated data matrices, storing experimental measurements (X) alongside observation metadata (obs), variable metadata (var), and multi-dimensional annotations (obsm, varm, obsp, varp, uns). Originally designed for single-cell genomics through Scanpy, it now serves as a general-purpose framework for any annotated data requiring efficient storage, manipulation, and analysis.

## When to Use This Skill

Use this skill when:
- Creating, reading, or writing AnnData objects
- Working with h5ad, zarr, or other genomics data formats
- Performing single-cell RNA-seq analysis
- Managing large datasets with sparse matrices or backed mode
- Concatenating multiple datasets or experimental batches
- Subsetting, filtering, or transforming annotated data
- Integrating with scanpy, scvi-tools, or other scverse ecosystem tools

## Installation

```bash
uv pip install anndata

# With optional dependencies
uv pip install anndata[dev,test,doc]
```

## Quick Start

### Creating an AnnData object
```python
import anndata as ad
import numpy as np
import pandas as pd

# Minimal creation
X = np.random.rand(100, 2000)  # 100 cells × 2000 genes
adata = ad.AnnData(X)

# With metadata
obs = pd.DataFrame({
    'cell_type': ['T cell', 'B cell'] * 50,
    'sample': ['A', 'B'] * 50
}, index=[f'cell_{i}' for i in range(100)])

var = pd.DataFrame({
    'gene_name': [f'Gene_{i}' for i in range(2000)]
}, index=[f'ENSG{i:05d}' for i in range(2000)])

adata = ad.AnnData(X=X, obs=obs, var=var)
```

### Reading data
```python
# Read h5ad file
adata = ad.read_h5ad('data.h5ad')

# Read with backed mode (for large files)
adata = ad.read_h5ad('large_data.h5ad', backed='r')

# Read other formats
adata = ad.read_csv('data.csv')
adata = ad.read_loom('data.loom')
adata = ad.read_10x_h5('filtered_feature_bc_matrix.h5')
```

### Writing data
```python
# Write h5ad file
adata.write_h5ad('output.h5ad')

# Write with compression
adata.write_h5ad('output.h5ad', compression='gzip')

# Write other formats
adata.write_zarr('output.zarr')
adata.write_csvs('output_dir/')
```

### Basic operations
```python
# Subset by conditions
t_cells = adata[adata.obs['cell_type'] == 'T cell']

# Subset by indices
subset = adata[0:50, 0:100]

# Add metadata
adata.obs['quality_score'] = np.random.rand(adata.n_obs)
adata.var['highly_variable'] = np.random.rand(adata.n_vars) > 0.8

# Access dimensions
print(f"{adata.n_obs} observations × {adata.n_vars} variables")
```

## Core Capabilities

### 1. Data Structure

Understand the AnnData object structure including X, obs, var, layers, obsm, varm, obsp, varp, uns, and raw components.

**See**: `references/data_structure.md` for comprehensive information on:
- Core components (X, obs, var, layers, obsm, varm, obsp, varp, uns, raw)
- Creating AnnData objects from various sources
- Accessing and manipulating data components
- Memory-efficient practices

### 2. Input/Output Operations

Read and write data in variou

... (truncated from original)
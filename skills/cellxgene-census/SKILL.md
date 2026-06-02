---
name: cellxgene-census
description: Query the CELLxGENE Census (61M+ cells) programmatically. Use when you need expression data across tissues, diseases, or cell types from the largest curated single-cell atlas. Best for population-scal
triggers:
  - cellxgene census
  - genomic
  - bioinformatics
  - sequence analysis
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# CZ CELLxGENE Census

## Overview

The CZ CELLxGENE Census provides programmatic access to a comprehensive, versioned collection of standardized single-cell genomics data from CZ CELLxGENE Discover. This skill enables efficient querying and analysis of millions of cells across thousands of datasets.

The Census includes:
- **61+ million cells** from human and mouse
- **Standardized metadata** (cell types, tissues, diseases, donors)
- **Raw gene expression** matrices
- **Pre-calculated embeddings** and statistics
- **Integration with PyTorch, scanpy, and other analysis tools**

## When to Use This Skill

This skill should be used when:
- Querying single-cell expression data by cell type, tissue, or disease
- Exploring available single-cell datasets and metadata
- Training machine learning models on single-cell data
- Performing large-scale cross-dataset analyses
- Integrating Census data with scanpy or other analysis frameworks
- Computing statistics across millions of cells
- Accessing pre-calculated embeddings or model predictions

## Installation and Setup

Install the Census API:
```bash
uv pip install cellxgene-census
```

For machine learning workflows, install additional dependencies:
```bash
uv pip install cellxgene-census[experimental]
```

## Core Workflow Patterns

### 1. Opening the Census

Always use the context manager to ensure proper resource cleanup:

```python
import cellxgene_census

# Open latest stable version
with cellxgene_census.open_soma() as census:
    # Work with census data

# Open specific version for reproducibility
with cellxgene_census.open_soma(census_version="2023-07-25") as census:
    # Work with census data
```

**Key points:**
- Use context manager (`with` statement) for automatic cleanup
- Specify `census_version` for reproducible analyses
- Default opens latest "stable" release

### 2. Exploring Census Information

Before querying expression data, explore available datasets and metadata.

**Access summary information:**
```python
# Get summary statistics
summary = census["census_info"]["summary"].read().concat().to_pandas()
print(f"Total cells: {summary['total_cell_count'][0]}")

# Get all datasets
datasets = census["census_info"]["datasets"].read().concat().to_pandas()

# Filter datasets by criteria
covid_datasets = datasets[datasets["disease"].str.contains("COVID", na=False)]
```

**Query cell metadata to understand available data:**
```python
# Get unique cell types in a tissue
cell_metadata = cellxgene_census.get_obs(
    census,
    "homo_sapiens",
    value_filter="tissue_general == 'brain' and is_primary_data == True",
    column_names=["cell_type"]
)
unique_cell_types = cell_metadata["cell_type"].unique()
print(f"Found {len(unique_cell_types)} cell types in brain")

# Count cells by tissue
tissue_counts = cell_metadata.groupby("tissue_general").size()
```

**Important:** Always filter for `is_primary_data == True` to avoid counting duplicate cells unless specifically analyzing duplicates.

### 3. Q

... (truncated from original)
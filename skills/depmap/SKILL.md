---
name: depmap
description: Query the Cancer Dependency Map (DepMap) for cancer cell line gene dependency scores (CRISPR Chronos), drug sensitivity data, and gene effect profiles. Use for identifying cancer-specific vulnerabilit
triggers:
  - depmap
tools_needed:
version: "0.1.0"
author: Kuan-lin Huang
priority: 5
---

# DepMap — Cancer Dependency Map

## Overview

The Cancer Dependency Map (DepMap) project, run by the Broad Institute, systematically characterizes genetic dependencies across hundreds of cancer cell lines using genome-wide CRISPR knockout screens (DepMap CRISPR), RNA interference (RNAi), and compound sensitivity assays (PRISM). DepMap data is essential for:
- Identifying which genes are essential for specific cancer types
- Finding cancer-selective dependencies (therapeutic targets)
- Validating oncology drug targets
- Discovering synthetic lethal interactions

**Key resources:**
- DepMap Portal: https://depmap.org/portal/
- DepMap data downloads: https://depmap.org/portal/download/all/
- Python package: `depmap` (or access via API/downloads)
- API: https://depmap.org/portal/api/

## When to Use This Skill

Use DepMap when:

- **Target validation**: Is a gene essential for survival in cancer cell lines with a specific mutation (e.g., KRAS-mutant)?
- **Biomarker discovery**: What genomic features predict sensitivity to knockout of a gene?
- **Synthetic lethality**: Find genes that are selectively essential when another gene is mutated/deleted
- **Drug sensitivity**: What cell line features predict response to a compound?
- **Pan-cancer essentiality**: Is a gene broadly essential across all cancer types (bad target) or selectively essential?
- **Correlation analysis**: Which pairs of genes have correlated dependency profiles (co-essentiality)?

## Core Concepts

### Dependency Scores

| Score | Range | Meaning |
|-------|-------|---------|
| **Chronos** (CRISPR) | ~ -3 to 0+ | More negative = more essential. Common essential threshold: −1. Pan-essential genes ~−1 to −2 |
| **RNAi DEMETER2** | ~ -3 to 0+ | Similar scale to Chronos |
| **Gene Effect** | normalized | Normalized Chronos; −1 = median effect of common essential genes |

**Key thresholds:**
- Chronos ≤ −0.5: likely dependent
- Chronos ≤ −1: strongly dependent (common essential range)

### Cell Line Annotations

Each cell line has:
- `DepMap_ID`: unique identifier (e.g., `ACH-000001`)
- `cell_line_name`: human-readable name
- `primary_disease`: cancer type
- `lineage`: broad tissue lineage
- `lineage_subtype`: specific subtype

## Core Capabilities

### 1. DepMap API

```python
import requests
import pandas as pd

BASE_URL = "https://depmap.org/portal/api"

def depmap_get(endpoint, params=None):
    url = f"{BASE_URL}/{endpoint}"
    response = requests.get(url, params=params)
    response.raise_for_status()
    return response.json()
```

### 2. Gene Dependency Scores

```python
def get_gene_dependency(gene_symbol, dataset="Chronos_Combined"):
    """Get CRISPR dependency scores for a gene across all cell lines."""
    url = f"{BASE_URL}/gene"
    params = {
        "gene_id": gene_symbol,
        "dataset": dataset
    }
    response = requests.get(url, params=params)
    return response.json()

# Alternatively, use the /data endpoint:
def get_dependencies_slice(gene_symbol, dataset_nam

... (truncated from original)
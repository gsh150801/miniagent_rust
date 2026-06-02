---
name: umap-learn
description: UMAP dimensionality reduction. Fast nonlinear manifold learning for 2D/3D visualization, clustering preprocessing (HDBSCAN), supervised/parametric UMAP, for high-dimensional data.
triggers:
  - machine learning
  - umap learn
  - train model
  - predict
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# UMAP-Learn

## Overview

UMAP (Uniform Manifold Approximation and Projection) is a dimensionality reduction technique for visualization and general non-linear dimensionality reduction. Apply this skill for fast, scalable embeddings that preserve local and global structure, supervised learning, and clustering preprocessing.

## Quick Start

### Installation

```bash
uv pip install umap-learn
```

### Basic Usage

UMAP follows scikit-learn conventions and can be used as a drop-in replacement for t-SNE or PCA.

```python
import umap
from sklearn.preprocessing import StandardScaler

# Prepare data (standardization is essential)
scaled_data = StandardScaler().fit_transform(data)

# Method 1: Single step (fit and transform)
embedding = umap.UMAP().fit_transform(scaled_data)

# Method 2: Separate steps (for reusing trained model)
reducer = umap.UMAP(random_state=42)
reducer.fit(scaled_data)
embedding = reducer.embedding_  # Access the trained embedding
```

**Critical preprocessing requirement:** Always standardize features to comparable scales before applying UMAP to ensure equal weighting across dimensions.

### Typical Workflow

```python
import umap
import matplotlib.pyplot as plt
from sklearn.preprocessing import StandardScaler

# 1. Preprocess data
scaler = StandardScaler()
scaled_data = scaler.fit_transform(raw_data)

# 2. Create and fit UMAP
reducer = umap.UMAP(
    n_neighbors=15,
    min_dist=0.1,
    n_components=2,
    metric='euclidean',
    random_state=42
)
embedding = reducer.fit_transform(scaled_data)

# 3. Visualize
plt.scatter(embedding[:, 0], embedding[:, 1], c=labels, cmap='Spectral', s=5)
plt.colorbar()
plt.title('UMAP Embedding')
plt.show()
```

## Parameter Tuning Guide

UMAP has four primary parameters that control the embedding behavior. Understanding these is crucial for effective usage.

### n_neighbors (default: 15)

**Purpose:** Balances local versus global structure in the embedding.

**How it works:** Controls the size of the local neighborhood UMAP examines when learning manifold structure.

**Effects by value:**
- **Low values (2-5):** Emphasizes fine local detail but may fragment data into disconnected components
- **Medium values (15-20):** Balanced view of both local structure and global relationships (recommended starting point)
- **High values (50-200):** Prioritizes broad topological structure at the expense of fine-grained details

**Recommendation:** Start with 15 and adjust based on results. Increase for more global structure, decrease for more local detail.

### min_dist (default: 0.1)

**Purpose:** Controls how tightly points cluster in the low-dimensional space.

**How it works:** Sets the minimum distance apart that points are allowed to be in the output representation.

**Effects by value:**
- **Low values (0.0-0.1):** Creates clumped embeddings useful for clustering; reveals fine topological details
- **High values (0.5-0.99):** Prevents tight packing; emphasizes broad topological preservation over loca

... (truncated from original)
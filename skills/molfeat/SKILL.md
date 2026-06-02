---
name: molfeat
description: Molecular featurization for ML (100+ featurizers). ECFP, MACCS, descriptors, pretrained models (ChemBERTa), convert SMILES to features, for QSAR and molecular ML.
triggers:
  - molfeat
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Molfeat - Molecular Featurization Hub

## Overview

Molfeat is a comprehensive Python library for molecular featurization that unifies 100+ pre-trained embeddings and hand-crafted featurizers. Convert chemical structures (SMILES strings or RDKit molecules) into numerical representations for machine learning tasks including QSAR modeling, virtual screening, similarity searching, and deep learning applications. Features fast parallel processing, scikit-learn compatible transformers, and built-in caching.

## When to Use This Skill

This skill should be used when working with:
- **Molecular machine learning**: Building QSAR/QSPR models, property prediction
- **Virtual screening**: Ranking compound libraries for biological activity
- **Similarity searching**: Finding structurally similar molecules
- **Chemical space analysis**: Clustering, visualization, dimensionality reduction
- **Deep learning**: Training neural networks on molecular data
- **Featurization pipelines**: Converting SMILES to ML-ready representations
- **Cheminformatics**: Any task requiring molecular feature extraction

## Installation

```bash
uv pip install molfeat

# With all optional dependencies
uv pip install "molfeat[all]"
```

**Optional dependencies for specific featurizers:**
- `molfeat[dgl]` - GNN models (GIN variants)
- `molfeat[graphormer]` - Graphormer models
- `molfeat[transformer]` - ChemBERTa, ChemGPT, MolT5
- `molfeat[fcd]` - FCD descriptors
- `molfeat[map4]` - MAP4 fingerprints

## Core Concepts

Molfeat organizes featurization into three hierarchical classes:

### 1. Calculators (`molfeat.calc`)

Callable objects that convert individual molecules into feature vectors. Accept RDKit `Chem.Mol` objects or SMILES strings.

**Use calculators for:**
- Single molecule featurization
- Custom processing loops
- Direct feature computation

**Example:**
```python
from molfeat.calc import FPCalculator

calc = FPCalculator("ecfp", radius=3, fpSize=2048)
features = calc("CCO")  # Returns numpy array (2048,)
```

### 2. Transformers (`molfeat.trans`)

Scikit-learn compatible transformers that wrap calculators for batch processing with parallelization.

**Use transformers for:**
- Batch featurization of molecular datasets
- Integration with scikit-learn pipelines
- Parallel processing (automatic CPU utilization)

**Example:**
```python
from molfeat.trans import MoleculeTransformer
from molfeat.calc import FPCalculator

transformer = MoleculeTransformer(FPCalculator("ecfp"), n_jobs=-1)
features = transformer(smiles_list)  # Parallel processing
```

### 3. Pretrained Transformers (`molfeat.trans.pretrained`)

Specialized transformers for deep learning models with batched inference and caching.

**Use pretrained transformers for:**
- State-of-the-art molecular embeddings
- Transfer learning from large chemical datasets
- Deep learning feature extraction

**Example:**
```python
from molfeat.trans.pretrained import PretrainedMolTransformer

transformer = PretrainedMolTransformer("C

... (truncated from original)
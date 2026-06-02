---
name: torchdrug
description: PyTorch-native graph neural networks for molecules and proteins. Use when building custom GNN architectures for drug discovery, protein modeling, or knowledge graph reasoning. Best for custom model de
triggers:
  - torchdrug
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# TorchDrug

## Overview

TorchDrug is a comprehensive PyTorch-based machine learning toolbox for drug discovery and molecular science. Apply graph neural networks, pre-trained models, and task definitions to molecules, proteins, and biological knowledge graphs, including molecular property prediction, protein modeling, knowledge graph reasoning, molecular generation, retrosynthesis planning, with 40+ curated datasets and 20+ model architectures.

## When to Use This Skill

This skill should be used when working with:

**Data Types:**
- SMILES strings or molecular structures
- Protein sequences or 3D structures (PDB files)
- Chemical reactions and retrosynthesis
- Biomedical knowledge graphs
- Drug discovery datasets

**Tasks:**
- Predicting molecular properties (solubility, toxicity, activity)
- Protein function or structure prediction
- Drug-target binding prediction
- Generating new molecular structures
- Planning chemical synthesis routes
- Link prediction in biomedical knowledge bases
- Training graph neural networks on scientific data

**Libraries and Integration:**
- TorchDrug is the primary library
- Often used with RDKit for cheminformatics
- Compatible with PyTorch and PyTorch Lightning
- Integrates with AlphaFold and ESM for proteins

## Getting Started

### Installation

```bash
uv pip install torchdrug
# Or with optional dependencies
uv pip install torchdrug[full]
```

### Quick Example

```python
from torchdrug import datasets, models, tasks
from torch.utils.data import DataLoader

# Load molecular dataset
dataset = datasets.BBBP("~/molecule-datasets/")
train_set, valid_set, test_set = dataset.split()

# Define GNN model
model = models.GIN(
    input_dim=dataset.node_feature_dim,
    hidden_dims=[256, 256, 256],
    edge_input_dim=dataset.edge_feature_dim,
    batch_norm=True,
    readout="mean"
)

# Create property prediction task
task = tasks.PropertyPrediction(
    model,
    task=dataset.tasks,
    criterion="bce",
    metric=["auroc", "auprc"]
)

# Train with PyTorch
optimizer = torch.optim.Adam(task.parameters(), lr=1e-3)
train_loader = DataLoader(train_set, batch_size=32, shuffle=True)

for epoch in range(100):
    for batch in train_loader:
        loss = task(batch)
        optimizer.zero_grad()
        loss.backward()
        optimizer.step()
```

## Core Capabilities

### 1. Molecular Property Prediction

Predict chemical, physical, and biological properties of molecules from structure.

**Use Cases:**
- Drug-likeness and ADMET properties
- Toxicity screening
- Quantum chemistry properties
- Binding affinity prediction

**Key Components:**
- 20+ molecular datasets (BBBP, HIV, Tox21, QM9, etc.)
- GNN models (GIN, GAT, SchNet)
- PropertyPrediction and MultipleBinaryClassification tasks

**Reference:** See `references/molecular_property_prediction.md` for:
- Complete dataset catalog
- Model selection guide
- Training workflows and best practices
- Feature engineering details

### 2. Protein Modeling

Work with protein sequen

... (truncated from original)
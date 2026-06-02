---
name: deepchem
description: Molecular ML with diverse featurizers and pre-built datasets. Use for property prediction (ADMET, toxicity) with traditional ML or GNNs when you want extensive featurization options and MoleculeNet be
triggers:
  - deepchem
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 7
---

# DeepChem

## Overview

DeepChem is a comprehensive Python library for applying machine learning to chemistry, materials science, and biology. Enable molecular property prediction, drug discovery, materials design, and biomolecule analysis through specialized neural networks, molecular featurization methods, and pretrained models.

## When to Use This Skill

This skill should be used when:
- Loading and processing molecular data (SMILES strings, SDF files, protein sequences)
- Predicting molecular properties (solubility, toxicity, binding affinity, ADMET properties)
- Training models on chemical/biological datasets
- Using MoleculeNet benchmark datasets (Tox21, BBBP, Delaney, etc.)
- Converting molecules to ML-ready features (fingerprints, graph representations, descriptors)
- Implementing graph neural networks for molecules (GCN, GAT, MPNN, AttentiveFP)
- Applying transfer learning with pretrained models (ChemBERTa, GROVER, MolFormer)
- Predicting crystal/materials properties (bandgap, formation energy)
- Analyzing protein or DNA sequences

## Core Capabilities

### 1. Molecular Data Loading and Processing

DeepChem provides specialized loaders for various chemical data formats:

```python
import deepchem as dc

# Load CSV with SMILES
featurizer = dc.feat.CircularFingerprint(radius=2, size=2048)
loader = dc.data.CSVLoader(
    tasks=['solubility', 'toxicity'],
    feature_field='smiles',
    featurizer=featurizer
)
dataset = loader.create_dataset('molecules.csv')

# Load SDF files
loader = dc.data.SDFLoader(tasks=['activity'], featurizer=featurizer)
dataset = loader.create_dataset('compounds.sdf')

# Load protein sequences
loader = dc.data.FASTALoader()
dataset = loader.create_dataset('proteins.fasta')
```

**Key Loaders**:
- `CSVLoader`: Tabular data with molecular identifiers
- `SDFLoader`: Molecular structure files
- `FASTALoader`: Protein/DNA sequences
- `ImageLoader`: Molecular images
- `JsonLoader`: JSON-formatted datasets

### 2. Molecular Featurization

Convert molecules into numerical representations for ML models.

#### Decision Tree for Featurizer Selection

```
Is the model a graph neural network?
├─ YES → Use graph featurizers
│   ├─ Standard GNN → MolGraphConvFeaturizer
│   ├─ Message passing → DMPNNFeaturizer
│   └─ Pretrained → GroverFeaturizer
│
└─ NO → What type of model?
    ├─ Traditional ML (RF, XGBoost, SVM)
    │   ├─ Fast baseline → CircularFingerprint (ECFP)
    │   ├─ Interpretable → RDKitDescriptors
    │   └─ Maximum coverage → MordredDescriptors
    │
    ├─ Deep learning (non-graph)
    │   ├─ Dense networks → CircularFingerprint
    │   └─ CNN → SmilesToImage
    │
    ├─ Sequence models (LSTM, Transformer)
    │   └─ SmilesToSeq
    │
    └─ 3D structure analysis
        └─ CoulombMatrix
```

#### Example Featurization

```python
# Fingerprints (for traditional ML)
fp = dc.feat.CircularFingerprint(radius=2, size=2048)

# Descriptors (for interpretable models)
desc = dc.feat.RDKitDescriptors()

# Graph features (f

... (truncated from original)
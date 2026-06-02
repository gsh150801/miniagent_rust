---
name: datamol
description: Pythonic wrapper around RDKit with simplified interface and sensible defaults. Preferred for standard drug discovery including SMILES parsing, standardization, descriptors, fingerprints, clustering, 3
triggers:
  - statistical test
  - datamol
  - analyze data
  - data analysis
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Datamol Cheminformatics Skill

## Overview

Datamol is a Python library that provides a lightweight, Pythonic abstraction layer over RDKit for molecular cheminformatics. Simplify complex molecular operations with sensible defaults, efficient parallelization, and modern I/O capabilities. All molecular objects are native `rdkit.Chem.Mol` instances, ensuring full compatibility with the RDKit ecosystem.

**Key capabilities**:
- Molecular format conversion (SMILES, SELFIES, InChI)
- Structure standardization and sanitization
- Molecular descriptors and fingerprints
- 3D conformer generation and analysis
- Clustering and diversity selection
- Scaffold and fragment analysis
- Chemical reaction application
- Visualization and alignment
- Batch processing with parallelization
- Cloud storage support via fsspec

## Installation and Setup

Guide users to install datamol:

```bash
uv pip install datamol
```

**Import convention**:
```python
import datamol as dm
```

## Core Workflows

### 1. Basic Molecule Handling

**Creating molecules from SMILES**:
```python
import datamol as dm

# Single molecule
mol = dm.to_mol("CCO")  # Ethanol

# From list of SMILES
smiles_list = ["CCO", "c1ccccc1", "CC(=O)O"]
mols = [dm.to_mol(smi) for smi in smiles_list]

# Error handling
mol = dm.to_mol("invalid_smiles")  # Returns None
if mol is None:
    print("Failed to parse SMILES")
```

**Converting molecules to SMILES**:
```python
# Canonical SMILES
smiles = dm.to_smiles(mol)

# Isomeric SMILES (includes stereochemistry)
smiles = dm.to_smiles(mol, isomeric=True)

# Other formats
inchi = dm.to_inchi(mol)
inchikey = dm.to_inchikey(mol)
selfies = dm.to_selfies(mol)
```

**Standardization and sanitization** (always recommend for user-provided molecules):
```python
# Sanitize molecule
mol = dm.sanitize_mol(mol)

# Full standardization (recommended for datasets)
mol = dm.standardize_mol(
    mol,
    disconnect_metals=True,
    normalize=True,
    reionize=True
)

# For SMILES strings directly
clean_smiles = dm.standardize_smiles(smiles)
```

### 2. Reading and Writing Molecular Files

Refer to `references/io_module.md` for comprehensive I/O documentation.

**Reading files**:
```python
# SDF files (most common in chemistry)
df = dm.read_sdf("compounds.sdf", mol_column='mol')

# SMILES files
df = dm.read_smi("molecules.smi", smiles_column='smiles', mol_column='mol')

# CSV with SMILES column
df = dm.read_csv("data.csv", smiles_column="SMILES", mol_column="mol")

# Excel files
df = dm.read_excel("compounds.xlsx", sheet_name=0, mol_column="mol")

# Universal reader (auto-detects format)
df = dm.open_df("file.sdf")  # Works with .sdf, .csv, .xlsx, .parquet, .json
```

**Writing files**:
```python
# Save as SDF
dm.to_sdf(mols, "output.sdf")
# Or from DataFrame
dm.to_sdf(df, "output.sdf", mol_column="mol")

# Save as SMILES file
dm.to_smi(mols, "output.smi")

# Excel with rendered molecule images
dm.to_xlsx(df, "output.xlsx", mol_columns=["mol"])
```

**Remote file support** (S3, GCS,

... (truncated from original)
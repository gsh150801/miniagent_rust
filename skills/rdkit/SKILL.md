---
name: rdkit
description: Cheminformatics toolkit for fine-grained molecular control. SMILES/SDF parsing, descriptors (MW, LogP, TPSA), fingerprints, substructure search, 2D/3D generation, similarity, reactions. For standard w
triggers:
  - rdkit
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 8
---

# RDKit Cheminformatics Toolkit

## Overview

RDKit is a comprehensive cheminformatics library providing Python APIs for molecular analysis and manipulation. This skill provides guidance for reading/writing molecular structures, calculating descriptors, fingerprinting, substructure searching, chemical reactions, 2D/3D coordinate generation, and molecular visualization. Use this skill for drug discovery, computational chemistry, and cheminformatics research tasks.

## Core Capabilities

### 1. Molecular I/O and Creation

**Reading Molecules:**

Read molecular structures from various formats:

```python
from rdkit import Chem

# From SMILES strings
mol = Chem.MolFromSmiles('Cc1ccccc1')  # Returns Mol object or None

# From MOL files
mol = Chem.MolFromMolFile('path/to/file.mol')

# From MOL blocks (string data)
mol = Chem.MolFromMolBlock(mol_block_string)

# From InChI
mol = Chem.MolFromInchi('InChI=1S/C6H6/c1-2-4-6-5-3-1/h1-6H')
```

**Writing Molecules:**

Convert molecules to text representations:

```python
# To canonical SMILES
smiles = Chem.MolToSmiles(mol)

# To MOL block
mol_block = Chem.MolToMolBlock(mol)

# To InChI
inchi = Chem.MolToInchi(mol)
```

**Batch Processing:**

For processing multiple molecules, use Supplier/Writer objects:

```python
# Read SDF files
suppl = Chem.SDMolSupplier('molecules.sdf')
for mol in suppl:
    if mol is not None:  # Check for parsing errors
        # Process molecule
        pass

# Read SMILES files
suppl = Chem.SmilesMolSupplier('molecules.smi', titleLine=False)

# For large files or compressed data
import gzip

with gzip.open('molecules.sdf.gz') as f:
    suppl = Chem.ForwardSDMolSupplier(f)
    for mol in suppl:
        # Process molecule
        pass

# Multithreaded processing for large datasets
suppl = Chem.MultithreadedSDMolSupplier('molecules.sdf')

# Write molecules to SDF
writer = Chem.SDWriter('output.sdf')
for mol in molecules:
    writer.write(mol)
writer.close()
```

**Important Notes:**
- All `MolFrom*` functions return `None` on failure with error messages
- Always check for `None` before processing molecules
- Molecules are automatically sanitized on import (validates valence, perceives aromaticity)

### 2. Molecular Sanitization and Validation

RDKit automatically sanitizes molecules during parsing, executing 13 steps including valence checking, aromaticity perception, and chirality assignment.

**Sanitization Control:**

```python
# Disable automatic sanitization
mol = Chem.MolFromSmiles('C1=CC=CC=C1', sanitize=False)

# Manual sanitization
Chem.SanitizeMol(mol)

# Detect problems before sanitization
problems = Chem.DetectChemistryProblems(mol)
for problem in problems:
    print(problem.GetType(), problem.Message())

# Partial sanitization (skip specific steps)
from rdkit.Chem import rdMolStandardize
Chem.SanitizeMol(mol, sanitizeOps=Chem.SANITIZE_ALL ^ Chem.SANITIZE_PROPERTIES)
```

**Common Sanitization Issues:**
- Atoms with explicit valence exceeding maximum allowed will raise excep

... (truncated from original)
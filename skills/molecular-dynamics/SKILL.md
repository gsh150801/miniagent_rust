---
name: molecular-dynamics
description: Run and analyze molecular dynamics simulations with OpenMM and MDAnalysis. Set up protein/small molecule systems, define force fields, run energy minimization and production MD, analyze trajectories (
triggers:
  - molecular dynamics
tools_needed:
version: "0.1.0"
author: Kuan-lin Huang
priority: 5
---

# Molecular Dynamics

## Overview

Molecular dynamics (MD) simulation computationally models the time evolution of molecular systems by integrating Newton's equations of motion. This skill covers two complementary tools:

- **OpenMM** (https://openmm.org/): High-performance MD simulation engine with GPU support, Python API, and flexible force field support
- **MDAnalysis** (https://mdanalysis.org/): Python library for reading, writing, and analyzing MD trajectories from all major simulation packages

**Installation:**
```bash
conda install -c conda-forge openmm mdanalysis nglview
# or
pip install openmm mdanalysis
```

## When to Use This Skill

Use molecular dynamics when:

- **Protein stability analysis**: How does a mutation affect protein dynamics?
- **Drug binding simulations**: Characterize binding mode and residence time of a ligand
- **Conformational sampling**: Explore protein flexibility and conformational changes
- **Protein-protein interaction**: Model interface dynamics and binding energetics
- **RMSD/RMSF analysis**: Quantify structural fluctuations from a reference structure
- **Free energy estimation**: Compute binding free energy or conformational free energy
- **Membrane simulations**: Model proteins in lipid bilayers
- **Intrinsically disordered proteins**: Study IDR conformational ensembles

## Core Workflow: OpenMM Simulation

### 1. System Preparation

```python
from openmm.app import *
from openmm import *
from openmm.unit import *
import sys

def prepare_system_from_pdb(pdb_file, forcefield_name="amber14-all.xml",
                              water_model="amber14/tip3pfb.xml"):
    """
    Prepare an OpenMM system from a PDB file.

    Args:
        pdb_file: Path to cleaned PDB file (use PDBFixer for raw PDB files)
        forcefield_name: Force field XML file
        water_model: Water model XML file

    Returns:
        pdb, forcefield, system, topology
    """
    # Load PDB
    pdb = PDBFile(pdb_file)

    # Load force field
    forcefield = ForceField(forcefield_name, water_model)

    # Add hydrogens and solvate
    modeller = Modeller(pdb.topology, pdb.positions)
    modeller.addHydrogens(forcefield)

    # Add solvent box (10 Å padding, 150 mM NaCl)
    modeller.addSolvent(
        forcefield,
        model='tip3p',
        padding=10*angstroms,
        ionicStrength=0.15*molar
    )

    print(f"System: {modeller.topology.getNumAtoms()} atoms, "
          f"{modeller.topology.getNumResidues()} residues")

    # Create system
    system = forcefield.createSystem(
        modeller.topology,
        nonbondedMethod=PME,         # Particle Mesh Ewald for long-range electrostatics
        nonbondedCutoff=1.0*nanometer,
        constraints=HBonds,           # Constrain hydrogen bonds (allows 2 fs timestep)
        rigidWater=True,
        ewaldErrorTolerance=0.0005
    )

    return modeller, system
```

### 2. Energy Minimization

```python
from openmm.app import *
from openmm import *
from openmm.unit import *

def

... (truncated from original)
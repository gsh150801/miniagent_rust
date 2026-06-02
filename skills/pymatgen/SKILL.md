---
name: pymatgen
description: Materials science toolkit. Crystal structures (CIF, POSCAR), phase diagrams, band structure, DOS, Materials Project integration, format conversion, for computational materials science.
triggers:
  - pymatgen
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Pymatgen - Python Materials Genomics

## Overview

Pymatgen is a comprehensive Python library for materials analysis that powers the Materials Project. Create, analyze, and manipulate crystal structures and molecules, compute phase diagrams and thermodynamic properties, analyze electronic structure (band structures, DOS), generate surfaces and interfaces, and access Materials Project's database of computed materials. Supports 100+ file formats from various computational codes.

## When to Use This Skill

This skill should be used when:
- Working with crystal structures or molecular systems in materials science
- Converting between structure file formats (CIF, POSCAR, XYZ, etc.)
- Analyzing symmetry, space groups, or coordination environments
- Computing phase diagrams or assessing thermodynamic stability
- Analyzing electronic structure data (band gaps, DOS, band structures)
- Generating surfaces, slabs, or studying interfaces
- Accessing the Materials Project database programmatically
- Setting up high-throughput computational workflows
- Analyzing diffusion, magnetism, or mechanical properties
- Working with VASP, Gaussian, Quantum ESPRESSO, or other computational codes

## Quick Start Guide

### Installation

```bash
# Core pymatgen
uv pip install pymatgen

# With Materials Project API access
uv pip install pymatgen mp-api

# Optional dependencies for extended functionality
uv pip install pymatgen[analysis]  # Additional analysis tools
uv pip install pymatgen[vis]       # Visualization tools
```

### Basic Structure Operations

```python
from pymatgen.core import Structure, Lattice

# Read structure from file (automatic format detection)
struct = Structure.from_file("POSCAR")

# Create structure from scratch
lattice = Lattice.cubic(3.84)
struct = Structure(lattice, ["Si", "Si"], [[0,0,0], [0.25,0.25,0.25]])

# Write to different format
struct.to(filename="structure.cif")

# Basic properties
print(f"Formula: {struct.composition.reduced_formula}")
print(f"Space group: {struct.get_space_group_info()}")
print(f"Density: {struct.density:.2f} g/cm³")
```

### Materials Project Integration

```bash
# Set up API key
export MP_API_KEY="your_api_key_here"
```

```python
from mp_api.client import MPRester

with MPRester() as mpr:
    # Get structure by material ID
    struct = mpr.get_structure_by_material_id("mp-149")

    # Search for materials
    materials = mpr.materials.summary.search(
        formula="Fe2O3",
        energy_above_hull=(0, 0.05)
    )
```

## Core Capabilities

### 1. Structure Creation and Manipulation

Create structures using various methods and perform transformations.

**From files:**
```python
# Automatic format detection
struct = Structure.from_file("structure.cif")
struct = Structure.from_file("POSCAR")
mol = Molecule.from_file("molecule.xyz")
```

**From scratch:**
```python
from pymatgen.core import Structure, Lattice

# Using lattice parameters
lattice = Lattice.from_parameters(a=3.84, b=3.84, c=3.84,
                  

... (truncated from original)
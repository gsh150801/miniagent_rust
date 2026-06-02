---
name: cobrapy
description: Constraint-based metabolic modeling (COBRA). FBA, FVA, gene knockouts, flux sampling, SBML models, for systems biology and metabolic engineering analysis.
triggers:
  - cobrapy
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# COBRApy - Constraint-Based Reconstruction and Analysis

## Overview

COBRApy is a Python library for constraint-based reconstruction and analysis (COBRA) of metabolic models, essential for systems biology research. Work with genome-scale metabolic models, perform computational simulations of cellular metabolism, conduct metabolic engineering analyses, and predict phenotypic behaviors.

## Core Capabilities

COBRApy provides comprehensive tools organized into several key areas:

### 1. Model Management

Load existing models from repositories or files:
```python
from cobra.io import load_model

# Load bundled test models
model = load_model("textbook")  # E. coli core model
model = load_model("ecoli")     # Full E. coli model
model = load_model("salmonella")

# Load from files
from cobra.io import read_sbml_model, load_json_model, load_yaml_model
model = read_sbml_model("path/to/model.xml")
model = load_json_model("path/to/model.json")
model = load_yaml_model("path/to/model.yml")
```

Save models in various formats:
```python
from cobra.io import write_sbml_model, save_json_model, save_yaml_model
write_sbml_model(model, "output.xml")  # Preferred format
save_json_model(model, "output.json")  # For Escher compatibility
save_yaml_model(model, "output.yml")   # Human-readable
```

### 2. Model Structure and Components

Access and inspect model components:
```python
# Access components
model.reactions      # DictList of all reactions
model.metabolites    # DictList of all metabolites
model.genes          # DictList of all genes

# Get specific items by ID or index
reaction = model.reactions.get_by_id("PFK")
metabolite = model.metabolites[0]

# Inspect properties
print(reaction.reaction)        # Stoichiometric equation
print(reaction.bounds)          # Flux constraints
print(reaction.gene_reaction_rule)  # GPR logic
print(metabolite.formula)       # Chemical formula
print(metabolite.compartment)   # Cellular location
```

### 3. Flux Balance Analysis (FBA)

Perform standard FBA simulation:
```python
# Basic optimization
solution = model.optimize()
print(f"Objective value: {solution.objective_value}")
print(f"Status: {solution.status}")

# Access fluxes
print(solution.fluxes["PFK"])
print(solution.fluxes.head())

# Fast optimization (objective value only)
objective_value = model.slim_optimize()

# Change objective
model.objective = "ATPM"
solution = model.optimize()
```

Parsimonious FBA (minimize total flux):
```python
from cobra.flux_analysis import pfba
solution = pfba(model)
```

Geometric FBA (find central solution):
```python
from cobra.flux_analysis import geometric_fba
solution = geometric_fba(model)
```

### 4. Flux Variability Analysis (FVA)

Determine flux ranges for all reactions:
```python
from cobra.flux_analysis import flux_variability_analysis

# Standard FVA
fva_result = flux_variability_analysis(model)

# FVA at 90% optimality
fva_result = flux_variability_analysis(model, fraction_of_optimum=0.9)

# Loopless FVA (eliminates thermodyna

... (truncated from original)
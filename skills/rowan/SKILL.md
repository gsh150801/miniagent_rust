---
name: rowan
description: Rowan is a cloud-native molecular modeling and medicinal-chemistry workflow platform with a Python API. Use for pKa and macropKa prediction, conformer and tautomer ensembles, docking and analogue dock
triggers:
  - rowan
tools_needed:
version: "0.1.0"
author: Rowan Science
priority: 5
---

# Rowan: Cloud-Native Molecular-Modeling and Drug-Design Workflows

## Overview

Rowan is a cloud-native workflow platform for molecular simulation, medicinal chemistry, and structure-based design. Its Python API exposes a unified interface for small-molecule modeling, property prediction, docking, molecular dynamics, and AI structure workflows.

Use Rowan when you want to run medicinal-chemistry or molecular-design workflows programmatically without maintaining local HPC infrastructure, GPU provisioning, or a collection of separate modeling tools. Rowan handles all infrastructure, result management, and computation scaling.

## When to use Rowan

**Rowan is a good fit for:**

- Quantum chemistry, semiempirical methods, or neural network potentials
- Batch property prediction (pKa, descriptors, permeability, solubility)
- Conformer and tautomer ensemble generation
- Docking workflows (single-ligand, analogue series, pose refinement)
- Protein-ligand cofolding and MSA generation
- Multi-step chemistry pipelines (e.g., tautomer search → docking → pose analysis)
- Batch medicinal-chemistry campaigns where you need consistent, scalable infrastructure

**Rowan is not the right fit for:**
- Simple molecular I/O (use RDKit directly)
- Post-HF *ab initio* quantum chemistry or relativistic calculations

## Access and pricing model

Rowan uses a credit-based usage model. All users, including free-tier users, can create API keys and use the Python API.

### Free-tier access

- Access to all Rowan core workflows
- 20 credits per week
- 500 signup credits

### Pricing and credit consumption

Credits are consumed according to compute type:

- **CPU**: 1 credit per minute
- **GPU**: 3 credits per minute
- **H100/H200 GPU**: 7 credits per minute

Purchased credits are priced per credit and remain valid for up to one year from purchase.

### Typical cost estimates

| Workflow | Typical Runtime | Estimated Credits | Notes |
|----------|----------------|-------------------|-------|
| Descriptors | <1 min | 0.5–2 | Lightweight, good for triage |
| pKa (single transition) | 2–5 min | 2–5 | Depends on molecule size |
| MacropKa (pH 0–14) | 5–15 min | 5–15 | Broader sampling, higher cost |
| Conformer search | 3–10 min | 3–10 | Ensemble quality matters |
| Tautomer search | 2–5 min | 2–5 | Heterocyclic systems |
| Docking (single ligand) | 5–20 min | 5–20 | Depends on pocket size, refinement |
| Analogue docking series (10–50 ligands) | 30–120 min | 30–100+ | Shared reference frame |
| MSA generation | 5–30 min | 5–30 | Sequence length dependent |
| Protein-ligand cofolding | 15–60 min | 20–50+ | AI structure prediction, GPU-heavy |

## Quick start

```bash
uv pip install rowan-python
```

```python
import rowan
rowan.api_key = "your_api_key_here"  # or set ROWAN_API_KEY env var

# Submit a descriptors workflow — completes in under a minute
wf = rowan.submit_descriptors_workflow("CC(=O)Oc1ccccc1C(=O)O", name="aspirin")
result = wf.result()

print(result.descriptors['MW

... (truncated from original)
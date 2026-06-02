---
name: glycoengineering
description: Analyze and engineer protein glycosylation. Scan sequences for N-glycosylation sequons (N-X-S/T), predict O-glycosylation hotspots, and access curated glycoengineering tools (NetOGlyc, GlycoShield, Gl
triggers:
  - glycoengineering
tools_needed:
version: "0.1.0"
author: Kuan-lin Huang
priority: 5
---

# Glycoengineering

## Overview

Glycosylation is the most common and complex post-translational modification (PTM) of proteins, affecting over 50% of all human proteins. Glycans regulate protein folding, stability, immune recognition, receptor interactions, and pharmacokinetics of therapeutic proteins. Glycoengineering involves rational modification of glycosylation patterns for improved therapeutic efficacy, stability, or immune evasion.

**Two major glycosylation types:**
- **N-glycosylation**: Attached to asparagine (N) in the sequon N-X-[S/T] where X ≠ Proline; occurs in the ER/Golgi
- **O-glycosylation**: Attached to serine (S) or threonine (T); no strict consensus motif; primarily GalNAc initiation

## When to Use This Skill

Use this skill when:

- **Antibody engineering**: Optimize Fc glycosylation for enhanced ADCC, CDC, or reduced immunogenicity
- **Therapeutic protein design**: Identify glycosylation sites that affect half-life, stability, or immunogenicity
- **Vaccine antigen design**: Engineer glycan shields to focus immune responses on conserved epitopes
- **Biosimilar characterization**: Compare glycan patterns between reference and biosimilar
- **Drug target analysis**: Does glycosylation affect target engagement for a receptor?
- **Protein stability**: N-glycans often stabilize proteins; identify sites for stabilizing mutations

## N-Glycosylation Sequon Analysis

### Scanning for N-Glycosylation Sites

N-glycosylation occurs at the sequon **N-X-[S/T]** where X ≠ Proline.

```python
import re
from typing import List, Tuple

def find_n_glycosylation_sequons(sequence: str) -> List[dict]:
    """
    Scan a protein sequence for canonical N-linked glycosylation sequons.
    Motif: N-X-[S/T], where X ≠ Proline.

    Args:
        sequence: Single-letter amino acid sequence

    Returns:
        List of dicts with position (1-based), motif, and context
    """
    seq = sequence.upper()
    results = []
    i = 0
    while i <= len(seq) - 3:
        triplet = seq[i:i+3]
        if triplet[0] == 'N' and triplet[1] != 'P' and triplet[2] in {'S', 'T'}:
            context = seq[max(0, i-3):i+6]  # ±3 residue context
            results.append({
                'position': i + 1,   # 1-based
                'motif': triplet,
                'context': context,
                'sequon_type': 'NXS' if triplet[2] == 'S' else 'NXT'
            })
            i += 3
        else:
            i += 1
    return results

def summarize_glycosylation_sites(sequence: str, protein_name: str = "") -> str:
    """Generate a research log summary of N-glycosylation sites."""
    sequons = find_n_glycosylation_sequons(sequence)

    lines = [f"# N-Glycosylation Sequon Analysis: {protein_name or 'Protein'}"]
    lines.append(f"Sequence length: {len(sequence)}")
    lines.append(f"Total N-glycosylation sequons: {len(sequons)}")

    if sequons:
        lines.append(f"\nN-X-S sites: {sum(1 for s in sequons if s['sequon_type'] == 'NXS')}")
        lines.appen

... (truncated from original)
---
name: biopython
description: Comprehensive molecular biology toolkit. Use for sequence manipulation, file parsing (FASTA/GenBank/PDB), phylogenetics, and programmatic NCBI/PubMed access (Bio.Entrez). Best for batch processing, cu
triggers:
  - bioinformatics
  - genomic
  - biopython
  - sequence analysis
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 8
---

# Biopython: Computational Molecular Biology in Python

## Overview

Biopython is a comprehensive set of freely available Python tools for biological computation. It provides functionality for sequence manipulation, file I/O, database access, structural bioinformatics, phylogenetics, and many other bioinformatics tasks. The current version is **Biopython 1.85** (released January 2025), which supports Python 3 and requires NumPy.

## When to Use This Skill

Use this skill when:

- Working with biological sequences (DNA, RNA, or protein)
- Reading, writing, or converting biological file formats (FASTA, GenBank, FASTQ, PDB, mmCIF, etc.)
- Accessing NCBI databases (GenBank, PubMed, Protein, Gene, etc.) via Entrez
- Running BLAST searches or parsing BLAST results
- Performing sequence alignments (pairwise or multiple sequence alignments)
- Analyzing protein structures from PDB files
- Creating, manipulating, or visualizing phylogenetic trees
- Finding sequence motifs or analyzing motif patterns
- Calculating sequence statistics (GC content, molecular weight, melting temperature, etc.)
- Performing structural bioinformatics tasks
- Working with population genetics data
- Any other computational molecular biology task

## Core Capabilities

Biopython is organized into modular sub-packages, each addressing specific bioinformatics domains:

1. **Sequence Handling** - Bio.Seq and Bio.SeqIO for sequence manipulation and file I/O
2. **Alignment Analysis** - Bio.Align and Bio.AlignIO for pairwise and multiple sequence alignments
3. **Database Access** - Bio.Entrez for programmatic access to NCBI databases
4. **BLAST Operations** - Bio.Blast for running and parsing BLAST searches
5. **Structural Bioinformatics** - Bio.PDB for working with 3D protein structures
6. **Phylogenetics** - Bio.Phylo for phylogenetic tree manipulation and visualization
7. **Advanced Features** - Motifs, population genetics, sequence utilities, and more

## Installation and Setup

Install Biopython using pip (requires Python 3 and NumPy):

```python
uv pip install biopython
```

For NCBI database access, always set your email address (required by NCBI):

```python
from Bio import Entrez
Entrez.email = "your.email@example.com"

# Optional: API key for higher rate limits (10 req/s instead of 3 req/s)
Entrez.api_key = "your_api_key_here"
```

## Using This Skill

This skill provides comprehensive documentation organized by functionality area. When working on a task, consult the relevant reference documentation:

### 1. Sequence Handling (Bio.Seq & Bio.SeqIO)

**Reference:** `references/sequence_io.md`

Use for:
- Creating and manipulating biological sequences
- Reading and writing sequence files (FASTA, GenBank, FASTQ, etc.)
- Converting between file formats
- Extracting sequences from large files
- Sequence translation, transcription, and reverse complement
- Working with SeqRecord objects

**Quick example:**
```python
from Bio import SeqIO

# Read sequences from FASTA file
for record in

... (truncated from original)
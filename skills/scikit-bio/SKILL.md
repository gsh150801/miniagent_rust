---
name: scikit-bio
description: Biological data toolkit. Sequence analysis, alignments, phylogenetic trees, diversity metrics (alpha/beta, UniFrac), ordination (PCoA), PERMANOVA, FASTA/Newick I/O, for microbiome analysis.
triggers:
  - scikit bio
  - genomic
  - bioinformatics
  - sequence analysis
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# scikit-bio

## Overview

scikit-bio is a comprehensive Python library for working with biological data. Apply this skill for bioinformatics analyses spanning sequence manipulation, alignment, phylogenetics, microbial ecology, and multivariate statistics.

## When to Use This Skill

This skill should be used when the user:
- Works with biological sequences (DNA, RNA, protein)
- Needs to read/write biological file formats (FASTA, FASTQ, GenBank, Newick, BIOM, etc.)
- Performs sequence alignments or searches for motifs
- Constructs or analyzes phylogenetic trees
- Calculates diversity metrics (alpha/beta diversity, UniFrac distances)
- Performs ordination analysis (PCoA, CCA, RDA)
- Runs statistical tests on biological/ecological data (PERMANOVA, ANOSIM, Mantel)
- Analyzes microbiome or community ecology data
- Works with protein embeddings from language models
- Needs to manipulate biological data tables

## Core Capabilities

### 1. Sequence Manipulation

Work with biological sequences using specialized classes for DNA, RNA, and protein data.

**Key operations:**
- Read/write sequences from FASTA, FASTQ, GenBank, EMBL formats
- Sequence slicing, concatenation, and searching
- Reverse complement, transcription (DNA→RNA), and translation (RNA→protein)
- Find motifs and patterns using regex
- Calculate distances (Hamming, k-mer based)
- Handle sequence quality scores and metadata

**Common patterns:**
```python
import skbio

# Read sequences from file
seq = skbio.DNA.read('input.fasta')

# Sequence operations
rc = seq.reverse_complement()
rna = seq.transcribe()
protein = rna.translate()

# Find motifs
motif_positions = seq.find_with_regex('ATG[ACGT]{3}')

# Check for properties
has_degens = seq.has_degenerates()
seq_no_gaps = seq.degap()
```

**Important notes:**
- Use `DNA`, `RNA`, `Protein` classes for grammared sequences with validation
- Use `Sequence` class for generic sequences without alphabet restrictions
- Quality scores automatically loaded from FASTQ files into positional metadata
- Metadata types: sequence-level (ID, description), positional (per-base), interval (regions/features)

### 2. Sequence Alignment

Perform pairwise and multiple sequence alignments using dynamic programming algorithms.

**Key capabilities:**
- Global alignment (Needleman-Wunsch with semi-global variant)
- Local alignment (Smith-Waterman)
- Configurable scoring schemes (match/mismatch, gap penalties, substitution matrices)
- CIGAR string conversion
- Multiple sequence alignment storage and manipulation with `TabularMSA`

**Common patterns:**
```python
from skbio.alignment import local_pairwise_align_ssw, TabularMSA

# Pairwise alignment
alignment = local_pairwise_align_ssw(seq1, seq2)

# Access aligned sequences
msa = alignment.aligned_sequences

# Read multiple alignment from file
msa = TabularMSA.read('alignment.fasta', constructor=skbio.DNA)

# Calculate consensus
consensus = msa.consensus()
```

**Important notes:**
- Use `local_pairwise_align_ssw` for local

... (truncated from original)
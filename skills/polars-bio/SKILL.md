---
name: polars-bio
description: High-performance genomic interval operations and bioinformatics file I/O on Polars DataFrames. Overlap, nearest, merge, coverage, complement, subtract for BED/VCF/BAM/GFF intervals. Streaming, cloud-n
triggers:
  - sequence analysis
  - genomic
  - bioinformatics
  - polars bio
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 8
---

# polars-bio

## Overview

polars-bio is a high-performance Python library for genomic interval operations and bioinformatics file I/O, built on Polars, Apache Arrow, and Apache DataFusion. It provides a familiar DataFrame-centric API for interval arithmetic (overlap, nearest, merge, coverage, complement, subtract) and reading/writing common bioinformatics formats (BED, VCF, BAM, CRAM, GFF/GTF, FASTA, FASTQ).

Key value propositions:
- **6-38x faster** than bioframe on real-world genomic benchmarks
- **Streaming/out-of-core** support for large genomes via DataFusion
- **Cloud-native** file I/O (S3, GCS, Azure) with predicate pushdown
- **Two API styles**: functional (`pb.overlap(df1, df2)`) and method-chaining (`df1.lazy().pb.overlap(df2)`)
- **SQL interface** for genomic data via DataFusion SQL engine

## When to Use This Skill

Use this skill when:
- Performing genomic interval operations (overlap, nearest, merge, coverage, complement, subtract)
- Reading/writing bioinformatics file formats (BED, VCF, BAM, CRAM, GFF/GTF, FASTA, FASTQ)
- Processing large genomic datasets that don't fit in memory (streaming mode)
- Running SQL queries on genomic data files
- Migrating from bioframe to a faster alternative
- Computing read depth/pileup from BAM/CRAM files
- Working with Polars DataFrames containing genomic intervals

## Quick Start

### Installation

```bash
pip install polars-bio
# or
uv pip install polars-bio
```

For pandas compatibility:
```bash
pip install polars-bio[pandas]
```

### Basic Overlap Example

```python
import polars as pl
import polars_bio as pb

# Create two interval DataFrames
df1 = pl.DataFrame({
    "chrom": ["chr1", "chr1", "chr1"],
    "start": [1, 5, 22],
    "end":   [6, 9, 30],
})

df2 = pl.DataFrame({
    "chrom": ["chr1", "chr1"],
    "start": [3, 25],
    "end":   [8, 28],
})

# Functional API (returns LazyFrame by default)
result = pb.overlap(df1, df2)
result_df = result.collect()

# Get a DataFrame directly
result_df = pb.overlap(df1, df2, output_type="polars.DataFrame")

# Method-chaining API (via .pb accessor on LazyFrame)
result = df1.lazy().pb.overlap(df2)
result_df = result.collect()
```

### Reading a BED File

```python
import polars_bio as pb

# Eager read (loads entire file)
df = pb.read_bed("regions.bed")

# Lazy scan (streaming, for large files)
lf = pb.scan_bed("regions.bed")
result = lf.collect()
```

## Core Capabilities

### 1. Genomic Interval Operations

polars-bio provides 8 core interval operations for genomic range arithmetic. All operations accept Polars DataFrames with `chrom`, `start`, `end` columns (configurable). All operations return a `LazyFrame` by default (use `output_type="polars.DataFrame"` for eager results).

**Operations:**
- `overlap` / `count_overlaps` - Find or count overlapping intervals between two sets
- `nearest` - Find nearest intervals (with configurable `k`, `overlap`, `distance` params)
- `merge` - Merge overlapping/bookended intervals within a set
- `cluster` - Assign c

... (truncated from original)
---
name: pysam
description: Genomic file toolkit. Read/write SAM/BAM/CRAM alignments, VCF/BCF variants, FASTA/FASTQ sequences, extract regions, calculate coverage, for NGS data processing pipelines.
triggers:
  - pysam
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Pysam

## Overview

Pysam is a Python module for reading, manipulating, and writing genomic datasets. Read/write SAM/BAM/CRAM alignment files, VCF/BCF variant files, and FASTA/FASTQ sequences with a Pythonic interface to htslib. Query tabix-indexed files, perform pileup analysis for coverage, and execute samtools/bcftools commands.

## When to Use This Skill

This skill should be used when:
- Working with sequencing alignment files (BAM/CRAM)
- Analyzing genetic variants (VCF/BCF)
- Extracting reference sequences or gene regions
- Processing raw sequencing data (FASTQ)
- Calculating coverage or read depth
- Implementing bioinformatics analysis pipelines
- Quality control of sequencing data
- Variant calling and annotation workflows

## Quick Start

### Installation
```bash
uv pip install pysam
```

### Basic Examples

**Read alignment file:**
```python
import pysam

# Open BAM file and fetch reads in region
samfile = pysam.AlignmentFile("example.bam", "rb")
for read in samfile.fetch("chr1", 1000, 2000):
    print(f"{read.query_name}: {read.reference_start}")
samfile.close()
```

**Read variant file:**
```python
# Open VCF file and iterate variants
vcf = pysam.VariantFile("variants.vcf")
for variant in vcf:
    print(f"{variant.chrom}:{variant.pos} {variant.ref}>{variant.alts}")
vcf.close()
```

**Query reference sequence:**
```python
# Open FASTA and extract sequence
fasta = pysam.FastaFile("reference.fasta")
sequence = fasta.fetch("chr1", 1000, 2000)
print(sequence)
fasta.close()
```

## Core Capabilities

### 1. Alignment File Operations (SAM/BAM/CRAM)

Use the `AlignmentFile` class to work with aligned sequencing reads. This is appropriate for analyzing mapping results, calculating coverage, extracting reads, or quality control.

**Common operations:**
- Open and read BAM/SAM/CRAM files
- Fetch reads from specific genomic regions
- Filter reads by mapping quality, flags, or other criteria
- Write filtered or modified alignments
- Calculate coverage statistics
- Perform pileup analysis (base-by-base coverage)
- Access read sequences, quality scores, and alignment information

**Reference:** See `references/alignment_files.md` for detailed documentation on:
- Opening and reading alignment files
- AlignedSegment attributes and methods
- Region-based fetching with `fetch()`
- Pileup analysis for coverage
- Writing and creating BAM files
- Coordinate systems and indexing
- Performance optimization tips

### 2. Variant File Operations (VCF/BCF)

Use the `VariantFile` class to work with genetic variants from variant calling pipelines. This is appropriate for variant analysis, filtering, annotation, or population genetics.

**Common operations:**
- Read and write VCF/BCF files
- Query variants in specific regions
- Access variant information (position, alleles, quality)
- Extract genotype data for samples
- Filter variants by quality, allele frequency, or other criteria
- Annotate variants with additional information
- Subset samples or regions

**R

... (truncated from original)
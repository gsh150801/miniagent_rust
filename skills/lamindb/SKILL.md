---
name: lamindb
description: This skill should be used when working with LaminDB, an open-source data framework for biology that makes data queryable, traceable, reproducible, and FAIR. Use when managing biological datasets (scRN
triggers:
  - lamindb
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# LaminDB

## Overview

LaminDB is an open-source data framework for biology designed to make data queryable, traceable, reproducible, and FAIR (Findable, Accessible, Interoperable, Reusable). It provides a unified platform that combines lakehouse architecture, lineage tracking, feature stores, biological ontologies, LIMS (Laboratory Information Management System), and ELN (Electronic Lab Notebook) capabilities through a single Python API.

**Core Value Proposition:**
- **Queryability**: Search and filter datasets by metadata, features, and ontology terms
- **Traceability**: Automatic lineage tracking from raw data through analysis to results
- **Reproducibility**: Version control for data, code, and environment
- **FAIR Compliance**: Standardized annotations using biological ontologies

## When to Use This Skill

Use this skill when:

- **Managing biological datasets**: scRNA-seq, bulk RNA-seq, spatial transcriptomics, flow cytometry, multi-modal data, EHR data
- **Tracking computational workflows**: Notebooks, scripts, pipeline execution (Nextflow, Snakemake, Redun)
- **Curating and validating data**: Schema validation, standardization, ontology-based annotation
- **Working with biological ontologies**: Genes, proteins, cell types, tissues, diseases, pathways (via Bionty)
- **Building data lakehouses**: Unified query interface across multiple datasets
- **Ensuring reproducibility**: Automatic versioning, lineage tracking, environment capture
- **Integrating ML pipelines**: Connecting with Weights & Biases, MLflow, HuggingFace, scVI-tools
- **Deploying data infrastructure**: Setting up local or cloud-based data management systems
- **Collaborating on datasets**: Sharing curated, annotated data with standardized metadata

## Core Capabilities

LaminDB provides six interconnected capability areas, each documented in detail in the references folder.

### 1. Core Concepts and Data Lineage

**Core entities:**
- **Artifacts**: Versioned datasets (DataFrame, AnnData, Parquet, Zarr, etc.)
- **Records**: Experimental entities (samples, perturbations, instruments)
- **Runs & Transforms**: Computational lineage tracking (what code produced what data)
- **Features**: Typed metadata fields for annotation and querying

**Key workflows:**
- Create and version artifacts from files or Python objects
- Track notebook/script execution with `ln.track()` and `ln.finish()`
- Annotate artifacts with typed features
- Visualize data lineage graphs with `artifact.view_lineage()`
- Query by provenance (find all outputs from specific code/inputs)

**Reference:** `references/core-concepts.md` - Read this for detailed information on artifacts, records, runs, transforms, features, versioning, and lineage tracking.

### 2. Data Management and Querying

**Query capabilities:**
- Registry exploration and lookup with auto-complete
- Single record retrieval with `get()`, `one()`, `one_or_none()`
- Filtering with comparison operators (`__gt`, `__lte`, `__contains`, `__startswith`)


... (truncated from original)
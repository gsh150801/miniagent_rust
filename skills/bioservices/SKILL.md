---
name: bioservices
description: Unified Python interface to 40+ bioinformatics services. Use when querying multiple databases (UniProt, KEGG, ChEMBL, Reactome) in a single workflow with consistent API. Best for cross-database analys
triggers:
  - bioservices
  - genomic
  - bioinformatics
  - sequence analysis
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# BioServices

## Overview

BioServices is a Python package providing programmatic access to approximately 40 bioinformatics web services and databases. Retrieve biological data, perform cross-database queries, map identifiers, analyze sequences, and integrate multiple biological resources in Python workflows. The package handles both REST and SOAP/WSDL protocols transparently.

## When to Use This Skill

This skill should be used when:
- Retrieving protein sequences, annotations, or structures from UniProt, PDB, Pfam
- Analyzing metabolic pathways and gene functions via KEGG or Reactome
- Searching compound databases (ChEBI, ChEMBL, PubChem) for chemical information
- Converting identifiers between different biological databases (KEGG↔UniProt, compound IDs)
- Running sequence similarity searches (BLAST, MUSCLE alignment)
- Querying gene ontology terms (QuickGO, GO annotations)
- Accessing protein-protein interaction data (PSICQUIC, IntactComplex)
- Mining genomic data (BioMart, ArrayExpress, ENA)
- Integrating data from multiple bioinformatics resources in a single workflow

## Core Capabilities

### 1. Protein Analysis

Retrieve protein information, sequences, and functional annotations:

```python
from bioservices import UniProt

u = UniProt(verbose=False)

# Search for protein by name
results = u.search("ZAP70_HUMAN", frmt="tab", columns="id,genes,organism")

# Retrieve FASTA sequence
sequence = u.retrieve("P43403", "fasta")

# Map identifiers between databases
kegg_ids = u.mapping(fr="UniProtKB_AC-ID", to="KEGG", query="P43403")
```

**Key methods:**
- `search()`: Query UniProt with flexible search terms
- `retrieve()`: Get protein entries in various formats (FASTA, XML, tab)
- `mapping()`: Convert identifiers between databases

Reference: `references/services_reference.md` for complete UniProt API details.

### 2. Pathway Discovery and Analysis

Access KEGG pathway information for genes and organisms:

```python
from bioservices import KEGG

k = KEGG()
k.organism = "hsa"  # Set to human

# Search for organisms
k.lookfor_organism("droso")  # Find Drosophila species

# Find pathways by name
k.lookfor_pathway("B cell")  # Returns matching pathway IDs

# Get pathways containing specific genes
pathways = k.get_pathway_by_gene("7535", "hsa")  # ZAP70 gene

# Retrieve and parse pathway data
data = k.get("hsa04660")
parsed = k.parse(data)

# Extract pathway interactions
interactions = k.parse_kgml_pathway("hsa04660")
relations = interactions['relations']  # Protein-protein interactions

# Convert to Simple Interaction Format
sif_data = k.pathway2sif("hsa04660")
```

**Key methods:**
- `lookfor_organism()`, `lookfor_pathway()`: Search by name
- `get_pathway_by_gene()`: Find pathways containing genes
- `parse_kgml_pathway()`: Extract structured pathway data
- `pathway2sif()`: Get protein interaction networks

Reference: `references/workflow_patterns.md` for complete pathway analysis workflows.

### 3. Compound Database Searches

Search and cross-refe

... (truncated from original)
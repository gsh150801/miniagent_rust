---
name: etetoolkit
description: Phylogenetic tree toolkit (ETE). Tree manipulation (Newick/NHX), evolutionary event detection, orthology/paralogy, NCBI taxonomy, visualization (PDF/SVG), for phylogenomics.
triggers:
  - etetoolkit
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# ETE Toolkit Skill

## Overview

ETE (Environment for Tree Exploration) is a toolkit for phylogenetic and hierarchical tree analysis. Manipulate trees, analyze evolutionary events, visualize results, and integrate with biological databases for phylogenomic research and clustering analysis.

## Core Capabilities

### 1. Tree Manipulation and Analysis

Load, manipulate, and analyze hierarchical tree structures with support for:

- **Tree I/O**: Read and write Newick, NHX, PhyloXML, and NeXML formats
- **Tree traversal**: Navigate trees using preorder, postorder, or levelorder strategies
- **Topology modification**: Prune, root, collapse nodes, resolve polytomies
- **Distance calculations**: Compute branch lengths and topological distances between nodes
- **Tree comparison**: Calculate Robinson-Foulds distances and identify topological differences

**Common patterns:**

```python
from ete3 import Tree

# Load tree from file
tree = Tree("tree.nw", format=1)

# Basic statistics
print(f"Leaves: {len(tree)}")
print(f"Total nodes: {len(list(tree.traverse()))}")

# Prune to taxa of interest
taxa_to_keep = ["species1", "species2", "species3"]
tree.prune(taxa_to_keep, preserve_branch_length=True)

# Midpoint root
midpoint = tree.get_midpoint_outgroup()
tree.set_outgroup(midpoint)

# Save modified tree
tree.write(outfile="rooted_tree.nw")
```

Use `scripts/tree_operations.py` for command-line tree manipulation:

```bash
# Display tree statistics
python scripts/tree_operations.py stats tree.nw

# Convert format
python scripts/tree_operations.py convert tree.nw output.nw --in-format 0 --out-format 1

# Reroot tree
python scripts/tree_operations.py reroot tree.nw rooted.nw --midpoint

# Prune to specific taxa
python scripts/tree_operations.py prune tree.nw pruned.nw --keep-taxa "sp1,sp2,sp3"

# Show ASCII visualization
python scripts/tree_operations.py ascii tree.nw
```

### 2. Phylogenetic Analysis

Analyze gene trees with evolutionary event detection:

- **Sequence alignment integration**: Link trees to multiple sequence alignments (FASTA, Phylip)
- **Species naming**: Automatic or custom species extraction from gene names
- **Evolutionary events**: Detect duplication and speciation events using Species Overlap or tree reconciliation
- **Orthology detection**: Identify orthologs and paralogs based on evolutionary events
- **Gene family analysis**: Split trees by duplications, collapse lineage-specific expansions

**Workflow for gene tree analysis:**

```python
from ete3 import PhyloTree

# Load gene tree with alignment
tree = PhyloTree("gene_tree.nw", alignment="alignment.fasta")

# Set species naming function
def get_species(gene_name):
    return gene_name.split("_")[0]

tree.set_species_naming_function(get_species)

# Detect evolutionary events
events = tree.get_descendant_evol_events()

# Analyze events
for node in tree.traverse():
    if hasattr(node, "evoltype"):
        if node.evoltype == "D":
            print(f"Duplication at {node.name}")
        

... (truncated from original)
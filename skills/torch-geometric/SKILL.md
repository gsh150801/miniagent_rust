---
name: torch-geometric
description: "Guide for building Graph Neural Networks with PyTorch Geometric (PyG). Use this skill whenever the user asks about graph neural networks, GNNs, node classification, link prediction, graph classificat
triggers:
  - torch geometric
tools_needed:
version: "0.1.0"
priority: 5
---

# PyTorch Geometric (PyG)

PyG is the standard library for Graph Neural Networks built on PyTorch. It provides data structures for graphs, 60+ GNN layer implementations, scalable mini-batch training, and support for heterogeneous graphs.

Install: `uv add torch_geometric` (or `uv pip install torch_geometric`; requires PyTorch). Optional: `pyg-lib`, `torch-scatter`, `torch-sparse`, `torch-cluster` for accelerated ops.

## Core Concepts

### Graph Data: `Data` and `HeteroData`

A graph lives in a `Data` object. The key attributes:

```python
from torch_geometric.data import Data

data = Data(
    x=node_features,          # [num_nodes, num_node_features]
    edge_index=edge_index,     # [2, num_edges] — COO format, dtype=torch.long
    edge_attr=edge_features,   # [num_edges, num_edge_features]
    y=labels,                  # node-level [num_nodes, *] or graph-level [1, *]
    pos=positions,             # [num_nodes, num_dimensions] (for point clouds/spatial)
)
```

**`edge_index` format is critical**: it's a `[2, num_edges]` tensor where `edge_index[0]` = source nodes, `edge_index[1]` = target nodes. It is NOT a list of tuples. If you have edge pairs as rows, transpose and call `.contiguous()`:

```python
# If edges are [[src1, dst1], [src2, dst2], ...] — transpose first:
edge_index = edge_pairs.t().contiguous()
```

For undirected graphs, include both directions: edge (0,1) needs both `[0,1]` and `[1,0]` in edge_index.

For heterogeneous graphs, use `HeteroData` — see the Heterogeneous Graphs section below.

### Datasets

PyG bundles many standard datasets that auto-download and preprocess:

```python
from torch_geometric.datasets import Planetoid, TUDataset

# Single-graph node classification (Cora, Citeseer, Pubmed)
dataset = Planetoid(root='./data', name='Cora')
data = dataset[0]  # single graph with train/val/test masks

# Multi-graph classification (ENZYMES, MUTAG, IMDB-BINARY, etc.)
dataset = TUDataset(root='./data', name='ENZYMES')
# dataset[0], dataset[1], ... are individual graphs
```

Common datasets by task:
- **Node classification**: Planetoid (Cora/Citeseer/Pubmed), OGB (ogbn-arxiv, ogbn-products, ogbn-mag)
- **Graph classification**: TUDataset (MUTAG, ENZYMES, PROTEINS, IMDB-BINARY), OGB (ogbg-molhiv)
- **Link prediction**: OGB (ogbl-collab, ogbl-citation2)
- **Molecular**: QM7, QM9, MoleculeNet
- **Point cloud/mesh**: ShapeNet, ModelNet10/40, FAUST

### Transforms

Transforms preprocess or augment graph data, analogous to torchvision transforms:

```python
import torch_geometric.transforms as T

# Common transforms
T.NormalizeFeatures()    # Row-normalize node features to sum to 1
T.ToUndirected()         # Add reverse edges to make graph undirected
T.AddSelfLoops()         # Add self-loop edges
T.KNNGraph(k=6)          # Build k-NN graph from point cloud positions
T.RandomJitter(0.01)     # Random noise augmentation on positions
T.Compose([...])         # Chain multiple transforms

# Apply as pre_transform (once, saved to disk) o

... (truncated from original)
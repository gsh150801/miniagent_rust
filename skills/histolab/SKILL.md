---
name: histolab
description: Lightweight WSI tile extraction and preprocessing. Use for basic slide processing tissue detection, tile extraction, stain normalization for H&E images. Best for simple pipelines, dataset preparation,
triggers:
  - histolab
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Histolab

## Overview

Histolab is a Python library for processing whole slide images (WSI) in digital pathology. It automates tissue detection, extracts informative tiles from gigapixel images, and prepares datasets for deep learning pipelines. The library handles multiple WSI formats, implements sophisticated tissue segmentation, and provides flexible tile extraction strategies.

## Installation

```bash
uv pip install histolab
```

## Quick Start

Basic workflow for extracting tiles from a whole slide image:

```python
from histolab.slide import Slide
from histolab.tiler import RandomTiler

# Load slide
slide = Slide("slide.svs", processed_path="output/")

# Configure tiler
tiler = RandomTiler(
    tile_size=(512, 512),
    n_tiles=100,
    level=0,
    seed=42
)

# Preview tile locations
tiler.locate_tiles(slide, n_tiles=20)

# Extract tiles
tiler.extract(slide)
```

## Core Capabilities

### 1. Slide Management

Load, inspect, and work with whole slide images in various formats.

**Common operations:**
- Loading WSI files (SVS, TIFF, NDPI, etc.)
- Accessing slide metadata (dimensions, magnification, properties)
- Generating thumbnails for visualization
- Working with pyramidal image structures
- Extracting regions at specific coordinates

**Key classes:** `Slide`

**Reference:** `references/slide_management.md` contains comprehensive documentation on:
- Slide initialization and configuration
- Built-in sample datasets (prostate, ovarian, breast, heart, kidney tissues)
- Accessing slide properties and metadata
- Thumbnail generation and visualization
- Working with pyramid levels
- Multi-slide processing workflows

**Example workflow:**
```python
from histolab.slide import Slide
from histolab.data import prostate_tissue

# Load sample data
prostate_svs, prostate_path = prostate_tissue()

# Initialize slide
slide = Slide(prostate_path, processed_path="output/")

# Inspect properties
print(f"Dimensions: {slide.dimensions}")
print(f"Levels: {slide.levels}")
print(f"Magnification: {slide.properties.get('openslide.objective-power')}")

# Save thumbnail
slide.save_thumbnail()
```

### 2. Tissue Detection and Masks

Automatically identify tissue regions and filter background/artifacts.

**Common operations:**
- Creating binary tissue masks
- Detecting largest tissue region
- Excluding background and artifacts
- Custom tissue segmentation
- Removing pen annotations

**Key classes:** `TissueMask`, `BiggestTissueBoxMask`, `BinaryMask`

**Reference:** `references/tissue_masks.md` contains comprehensive documentation on:
- TissueMask: Segments all tissue regions using automated filters
- BiggestTissueBoxMask: Returns bounding box of largest tissue region (default)
- BinaryMask: Base class for custom mask implementations
- Visualizing masks with `locate_mask()`
- Creating custom rectangular and annotation-exclusion masks
- Mask integration with tile extraction
- Best practices and troubleshooting

**Example workflow:**
```python
from histolab.masks impo

... (truncated from original)
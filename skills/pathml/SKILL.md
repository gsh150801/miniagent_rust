---
name: pathml
description: Full-featured computational pathology toolkit. Use for advanced WSI analysis including multiplexed immunofluorescence (CODEX, Vectra), nucleus segmentation, tissue graph construction, and ML model tra
triggers:
  - machine learning
  - train model
  - pathml
  - predict
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# PathML

## Overview

PathML is a comprehensive Python toolkit for computational pathology workflows, designed to facilitate machine learning and image analysis for whole-slide pathology images. The framework provides modular, composable tools for loading diverse slide formats, preprocessing images, constructing spatial graphs, training deep learning models, and analyzing multiparametric imaging data from technologies like CODEX and multiplex immunofluorescence.

## When to Use This Skill

Apply this skill for:
- Loading and processing whole-slide images (WSI) in various proprietary formats
- Preprocessing H&E stained tissue images with stain normalization
- Nucleus detection, segmentation, and classification workflows
- Building cell and tissue graphs for spatial analysis
- Training or deploying machine learning models (HoVer-Net, HACTNet) on pathology data
- Analyzing multiparametric imaging (CODEX, Vectra, MERFISH) for spatial proteomics
- Quantifying marker expression from multiplex immunofluorescence
- Managing large-scale pathology datasets with HDF5 storage
- Tile-based analysis and stitching operations

## Core Capabilities

PathML provides six major capability areas documented in detail within reference files:

### 1. Image Loading & Formats

Load whole-slide images from 160+ proprietary formats including Aperio SVS, Hamamatsu NDPI, Leica SCN, Zeiss ZVI, DICOM, and OME-TIFF. PathML automatically handles vendor-specific formats and provides unified interfaces for accessing image pyramids, metadata, and regions of interest.

**See:** `references/image_loading.md` for supported formats, loading strategies, and working with different slide types.

### 2. Preprocessing Pipelines

Build modular preprocessing pipelines by composing transforms for image manipulation, quality control, stain normalization, tissue detection, and mask operations. PathML's Pipeline architecture enables reproducible, scalable preprocessing across large datasets.

**Key transforms:**
- `StainNormalizationHE` - Macenko/Vahadane stain normalization
- `TissueDetectionHE`, `NucleusDetectionHE` - Tissue/nucleus segmentation
- `MedianBlur`, `GaussianBlur` - Noise reduction
- `LabelArtifactTileHE` - Quality control for artifacts

**See:** `references/preprocessing.md` for complete transform catalog, pipeline construction, and preprocessing workflows.

### 3. Graph Construction

Construct spatial graphs representing cellular and tissue-level relationships. Extract features from segmented objects to create graph-based representations suitable for graph neural networks and spatial analysis.

**See:** `references/graphs.md` for graph construction methods, feature extraction, and spatial analysis workflows.

### 4. Machine Learning

Train and deploy deep learning models for nucleus detection, segmentation, and classification. PathML integrates PyTorch with pre-built models (HoVer-Net, HACTNet), custom DataLoaders, and ONNX support for inference.

**Key models:**
- **HoVer-Net** -

... (truncated from original)
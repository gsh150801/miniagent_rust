---
name: neuropixels-analysis
description: Neuropixels neural recording analysis. Load SpikeGLX/OpenEphys data, preprocess, motion correction, Kilosort4 spike sorting, quality metrics, Allen/IBL curation, AI-assisted visual analysis, for Neuro
triggers:
  - statistical test
  - data analysis
  - analyze data
  - neuropixels analysis
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Neuropixels Data Analysis

## Overview

Comprehensive toolkit for analyzing Neuropixels high-density neural recordings using current best practices from SpikeInterface, Allen Institute, and International Brain Laboratory (IBL). Supports the full workflow from raw data to publication-ready curated units.

## When to Use This Skill

This skill should be used when:
- Working with Neuropixels recordings (.ap.bin, .lf.bin, .meta files)
- Loading data from SpikeGLX, Open Ephys, or NWB formats
- Preprocessing neural recordings (filtering, CAR, bad channel detection)
- Detecting and correcting motion/drift in recordings
- Running spike sorting (Kilosort4, SpykingCircus2, Mountainsort5)
- Computing quality metrics (SNR, ISI violations, presence ratio)
- Curating units using Allen/IBL criteria
- Creating visualizations of neural data
- Exporting results to Phy or NWB

## Supported Hardware & Formats

| Probe | Electrodes | Channels | Notes |
|-------|-----------|----------|-------|
| Neuropixels 1.0 | 960 | 384 | Requires phase_shift correction |
| Neuropixels 2.0 (single) | 1280 | 384 | Denser geometry |
| Neuropixels 2.0 (4-shank) | 5120 | 384 | Multi-region recording |

| Format | Extension | Reader |
|--------|-----------|--------|
| SpikeGLX | `.ap.bin`, `.lf.bin`, `.meta` | `si.read_spikeglx()` |
| Open Ephys | `.continuous`, `.oebin` | `si.read_openephys()` |
| NWB | `.nwb` | `si.read_nwb()` |

## Quick Start

### Basic Import and Setup

```python
import spikeinterface.full as si
import neuropixels_analysis as npa

# Configure parallel processing
job_kwargs = dict(n_jobs=-1, chunk_duration='1s', progress_bar=True)
```

### Loading Data

```python
# SpikeGLX (most common)
recording = si.read_spikeglx('/path/to/data', stream_id='imec0.ap')

# Open Ephys (common for many labs)
recording = si.read_openephys('/path/to/Record_Node_101/')

# Check available streams
streams, ids = si.get_neo_streams('spikeglx', '/path/to/data')
print(streams)  # ['imec0.ap', 'imec0.lf', 'nidq']

# For testing with subset of data
recording = recording.frame_slice(0, int(60 * recording.get_sampling_frequency()))
```

### Complete Pipeline (One Command)

```python
# Run full analysis pipeline
results = npa.run_pipeline(
    recording,
    output_dir='output/',
    sorter='kilosort4',
    curation_method='allen',
)

# Access results
sorting = results['sorting']
metrics = results['metrics']
labels = results['labels']
```

## Standard Analysis Workflow

### 1. Preprocessing

```python
# Recommended preprocessing chain
rec = si.highpass_filter(recording, freq_min=400)
rec = si.phase_shift(rec)  # Required for Neuropixels 1.0
bad_ids, _ = si.detect_bad_channels(rec)
rec = rec.remove_channels(bad_ids)
rec = si.common_reference(rec, operator='median')

# Or use our wrapper
rec = npa.preprocess(recording)
```

### 2. Check and Correct Drift

```python
# Check for drift (always do this!)
motion_info = npa.estimate_motion(rec, preset='kilosort_like')
npa.plot_drift(rec, motion_info, o

... (truncated from original)
---
name: bids
description: >
triggers:
  - bids
tools_needed:
version: "0.1.0"
author: Yaroslav Halchenko
priority: 5
---

# Brain Imaging Data Structure (BIDS)

## Overview

The Brain Imaging Data Structure (BIDS) is a community standard for organizing and describing neuroscience and biomedical research datasets. It defines a consistent file naming convention, directory hierarchy, and metadata schema so that datasets are immediately understandable by humans and software tools alike. BIDS is governed by the BIDS Specification (currently v1.11.x) and is maintained by the community via the BIDS-Standard GitHub organization.

While BIDS originated for MRI, it has grown well beyond neuroimaging. The specification now covers 11 modalities spanning imaging, electrophysiology, and behavioral data:

- **Imaging**: MRI (structural, functional, diffusion, fieldmaps, perfusion/ASL), PET, microscopy
- **Electrophysiology**: EEG, MEG, iEEG (intracranial EEG), EMG
- **Other**: NIRS (near-infrared spectroscopy), motion capture, behavioral data (without imaging), MR spectroscopy

Active BEPs are extending BIDS further — notably BEP032 (microelectrode electrophysiology) will add support for extracellular recordings including Neuropixels probes, bringing BIDS to a prevalent methodology in animal neuroscience research (see also the neuropixels-analysis skill).

Adoption is required or strongly encouraged by major data repositories (OpenNeuro, DANDI), leading journals (NeuroImage, Human Brain Mapping, Scientific Data), and funding agencies (NIH, ERC).

The Python ecosystem for BIDS centers on **PyBIDS** (`pybids`) for querying and indexing BIDS datasets, and the **bids-validator** (Deno-based, available as PyPI package `bids-validator-deno` or via Deno directly) for compliance checking. Conversion from DICOM is typically done with **HeuDiConv**, **dcm2bids**, or **BIDScoin**.

## When to Use This Skill

Apply this skill when:
- Organizing raw neuroscience data (imaging, electrophysiology, behavioral) into BIDS-compliant directory structures
- Querying an existing BIDS dataset to find specific files by subject, session, task, run, or modality
- Validating a dataset against the BIDS specification before sharing or submission
- Converting DICOM data from scanners into BIDS format
- Writing or editing JSON sidecar metadata files
- Creating BIDS-compliant derivatives (preprocessed data, analysis outputs)
- Setting up a `dataset_description.json` for a new dataset
- Working with BIDS entities (subject, session, task, acquisition, run, etc.)
- Configuring `.bidsignore` to exclude files from validation
- Preparing data for upload to OpenNeuro, DANDI, or other BIDS-aware repositories

## Installation

```bash
# Core BIDS querying library
uv pip install pybids

# BIDS validator (Deno-based, installed via PyPI wrapper)
uv pip install bids-validator-deno
# Alternative: install directly via Deno
# deno install -g -A npm:bids-validator

# DICOM-to-BIDS converters (install as needed)
uv pip install heudiconv       # HeuDiConv - heuristic-based DICOM conversion
uv pip install dcm2bids        # dcm2bids

... (truncated from original)
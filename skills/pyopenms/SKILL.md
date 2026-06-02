---
name: pyopenms
description: Complete mass spectrometry analysis platform. Use for proteomics workflows feature detection, peptide identification, protein quantification, and complex LC-MS/MS pipelines. Supports extensive file fo
triggers:
  - pyopenms
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# PyOpenMS

## Overview

PyOpenMS provides Python bindings to the OpenMS library for computational mass spectrometry, enabling analysis of proteomics and metabolomics data. Use for handling mass spectrometry file formats, processing spectral data, detecting features, identifying peptides/proteins, and performing quantitative analysis.

## Installation

Install using uv:

```bash
uv pip install pyopenms
```

Verify installation:

```python
import pyopenms
print(pyopenms.__version__)
```

## Core Capabilities

PyOpenMS organizes functionality into these domains:

### 1. File I/O and Data Formats

Handle mass spectrometry file formats and convert between representations.

**Supported formats**: mzML, mzXML, TraML, mzTab, FASTA, pepXML, protXML, mzIdentML, featureXML, consensusXML, idXML

Basic file reading:

```python
import pyopenms as ms

# Read mzML file
exp = ms.MSExperiment()
ms.MzMLFile().load("data.mzML", exp)

# Access spectra
for spectrum in exp:
    mz, intensity = spectrum.get_peaks()
    print(f"Spectrum: {len(mz)} peaks")
```

**For detailed file handling**: See `references/file_io.md`

### 2. Signal Processing

Process raw spectral data with smoothing, filtering, centroiding, and normalization.

Basic spectrum processing:

```python
# Smooth spectrum with Gaussian filter
gaussian = ms.GaussFilter()
params = gaussian.getParameters()
params.setValue("gaussian_width", 0.1)
gaussian.setParameters(params)
gaussian.filterExperiment(exp)
```

**For algorithm details**: See `references/signal_processing.md`

### 3. Feature Detection

Detect and link features across spectra and samples for quantitative analysis.

```python
# Detect features
ff = ms.FeatureFinder()
ff.run("centroided", exp, features, params, ms.FeatureMap())
```

**For complete workflows**: See `references/feature_detection.md`

### 4. Peptide and Protein Identification

Integrate with search engines and process identification results.

**Supported engines**: Comet, Mascot, MSGFPlus, XTandem, OMSSA, Myrimatch

Basic identification workflow:

```python
# Load identification data
protein_ids = []
peptide_ids = []
ms.IdXMLFile().load("identifications.idXML", protein_ids, peptide_ids)

# Apply FDR filtering
fdr = ms.FalseDiscoveryRate()
fdr.apply(peptide_ids)
```

**For detailed workflows**: See `references/identification.md`

### 5. Metabolomics Analysis

Perform untargeted metabolomics preprocessing and analysis.

Typical workflow:
1. Load and process raw data
2. Detect features
3. Align retention times across samples
4. Link features to consensus map
5. Annotate with compound databases

**For complete metabolomics workflows**: See `references/metabolomics.md`

## Data Structures

PyOpenMS uses these primary objects:

- **MSExperiment**: Collection of spectra and chromatograms
- **MSSpectrum**: Single mass spectrum with m/z and intensity pairs
- **MSChromatogram**: Chromatographic trace
- **Feature**: Detected chromatographic peak with quality metrics
- **FeatureMap**: Collection 

... (truncated from original)
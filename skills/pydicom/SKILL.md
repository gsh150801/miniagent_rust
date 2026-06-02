---
name: pydicom
description: Python library for working with DICOM (Digital Imaging and Communications in Medicine) files. Use this skill when reading, writing, or modifying medical imaging data in DICOM format, extracting pixel 
triggers:
  - pydicom
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Pydicom

## Overview

Pydicom is a pure Python package for working with DICOM files, the standard format for medical imaging data. This skill provides guidance on reading, writing, and manipulating DICOM files, including working with pixel data, metadata, and various compression formats.

## When to Use This Skill

Use this skill when working with:
- Medical imaging files (CT, MRI, X-ray, ultrasound, PET, etc.)
- DICOM datasets requiring metadata extraction or modification
- Pixel data extraction and image processing from medical scans
- DICOM anonymization for research or data sharing
- Converting DICOM files to standard image formats
- Compressed DICOM data requiring decompression
- DICOM sequences and structured reports
- Multi-slice volume reconstruction
- PACS (Picture Archiving and Communication System) integration

## Installation

Install pydicom and common dependencies:

```bash
uv pip install pydicom
uv pip install pillow  # For image format conversion
uv pip install numpy   # For pixel array manipulation
uv pip install matplotlib  # For visualization
```

For handling compressed DICOM files, additional packages may be needed:

```bash
uv pip install pylibjpeg pylibjpeg-libjpeg pylibjpeg-openjpeg  # JPEG compression
uv pip install python-gdcm  # Alternative compression handler
```

## Core Workflows

### Reading DICOM Files

Read a DICOM file using `pydicom.dcmread()`:

```python
import pydicom

# Read a DICOM file
ds = pydicom.dcmread('path/to/file.dcm')

# Access metadata
print(f"Patient Name: {ds.PatientName}")
print(f"Study Date: {ds.StudyDate}")
print(f"Modality: {ds.Modality}")

# Display all elements
print(ds)
```

**Key points:**
- `dcmread()` returns a `Dataset` object
- Access data elements using attribute notation (e.g., `ds.PatientName`) or tag notation (e.g., `ds[0x0010, 0x0010]`)
- Use `ds.file_meta` to access file metadata like Transfer Syntax UID
- Handle missing attributes with `getattr(ds, 'AttributeName', default_value)` or `hasattr(ds, 'AttributeName')`

### Working with Pixel Data

Extract and manipulate image data from DICOM files:

```python
import pydicom
import numpy as np
import matplotlib.pyplot as plt

# Read DICOM file
ds = pydicom.dcmread('image.dcm')

# Get pixel array (requires numpy)
pixel_array = ds.pixel_array

# Image information
print(f"Shape: {pixel_array.shape}")
print(f"Data type: {pixel_array.dtype}")
print(f"Rows: {ds.Rows}, Columns: {ds.Columns}")

# Apply windowing for display (CT/MRI)
if hasattr(ds, 'WindowCenter') and hasattr(ds, 'WindowWidth'):
    from pydicom.pixel_data_handlers.util import apply_voi_lut
    windowed_image = apply_voi_lut(pixel_array, ds)
else:
    windowed_image = pixel_array

# Display image
plt.imshow(windowed_image, cmap='gray')
plt.title(f"{ds.Modality} - {ds.StudyDescription}")
plt.axis('off')
plt.show()
```

**Working with color images:**

```python
# RGB images have shape (rows, columns, 3)
if ds.PhotometricInterpretation == 'RGB':
    rgb_image = ds.pixel_array

... (truncated from original)
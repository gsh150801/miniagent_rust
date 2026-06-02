---
name: imaging-data-commons
description: Query and download public cancer imaging data from NCI Imaging Data Commons using idc-index. Use for accessing large-scale radiology (CT, MR, PET) and pathology datasets for AI training or research. N
triggers:
  - analyze data
  - data analysis
  - imaging data commons
  - statistical test
tools_needed:
version: "0.1.0"
author: Andrey Fedorov, @fedorov
priority: 5
---

# Imaging Data Commons

## Overview

Use the `idc-index` Python package to query and download public cancer imaging data from the National Cancer Institute Imaging Data Commons (IDC). No authentication required for data access.

**Current IDC Data Version: v23** (always verify with `IDCClient().get_idc_version()`)

**Primary tool:** `idc-index` ([GitHub](https://github.com/imagingdatacommons/idc-index))

**CRITICAL - Check package version and upgrade if needed (run this FIRST):**

```python
import idc_index

REQUIRED_VERSION = "0.11.14"  # Must match metadata.idc-index in this file
installed = idc_index.__version__

if installed < REQUIRED_VERSION:
    print(f"Upgrading idc-index from {installed} to {REQUIRED_VERSION}...")
    import subprocess
    subprocess.run(["pip3", "install", "--upgrade", "--break-system-packages", "idc-index"], check=True)
    print("Upgrade complete. Restart Python to use new version.")
else:
    print(f"idc-index {installed} meets requirement ({REQUIRED_VERSION})")
```

**Verify IDC data version and check current data scale:**

```python
from idc_index import IDCClient
client = IDCClient()

# Verify IDC data version (should be "v23")
print(f"IDC data version: {client.get_idc_version()}")

# Get collection count and total series
stats = client.sql_query("""
    SELECT
        COUNT(DISTINCT collection_id) as collections,
        COUNT(DISTINCT analysis_result_id) as analysis_results,
        COUNT(DISTINCT PatientID) as patients,
        COUNT(DISTINCT StudyInstanceUID) as studies,
        COUNT(DISTINCT SeriesInstanceUID) as series,
        SUM(instanceCount) as instances,
        SUM(series_size_MB)/1000000 as size_TB
    FROM index
""")
print(stats)
```

**Core workflow:**
1. Query metadata → `client.sql_query()`
2. Download DICOM files → `client.download_from_selection()`
3. Visualize in browser → `client.get_viewer_URL(seriesInstanceUID=...)`

## When to Use This Skill

- Finding publicly available radiology (CT, MR, PET) or pathology (slide microscopy) images
- Selecting image subsets by cancer type, modality, anatomical site, or other metadata
- Downloading DICOM data from IDC
- Checking data licenses before use in research or commercial applications
- Visualizing medical images in a browser without local DICOM viewer software

## Quick Navigation

**Core Sections (inline):**
- IDC Data Model - Collection and analysis result hierarchy
- Index Tables - Available tables and joining patterns
- Installation - Package setup and version verification
- Core Capabilities - Essential API patterns (query, download, visualize, license, citations, batch)
- Best Practices - Usage guidelines
- Troubleshooting - Common issues and solutions

**Reference Guides (load on demand):**

| Guide | When to Load |
|-------|--------------|
| `index_tables_guide.md` | Complex JOINs, schema discovery, DataFrame access |
| `use_cases.md` | End-to-end workflow examples (training datasets, batch downloads) |
| `sql_patterns.md` | Quick SQL patt

... (truncated from original)
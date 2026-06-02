---
name: flowio
description: Parse FCS (Flow Cytometry Standard) files v2.0-3.1. Extract events as NumPy arrays, read metadata/channels, convert to CSV/DataFrame, for flow cytometry data preprocessing.
triggers:
  - flowio
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# FlowIO: Flow Cytometry Standard File Handler

## Overview

FlowIO is a lightweight Python library for reading and writing Flow Cytometry Standard (FCS) files. Parse FCS metadata, extract event data, and create new FCS files with minimal dependencies. The library supports FCS versions 2.0, 3.0, and 3.1, making it ideal for backend services, data pipelines, and basic cytometry file operations.

## When to Use This Skill

This skill should be used when:

- FCS files requiring parsing or metadata extraction
- Flow cytometry data needing conversion to NumPy arrays
- Event data requiring export to FCS format
- Multi-dataset FCS files needing separation
- Channel information extraction (scatter, fluorescence, time)
- Cytometry file validation or inspection
- Pre-processing workflows before advanced analysis

**Related Tools:** For advanced flow cytometry analysis including compensation, gating, and FlowJo/GatingML support, recommend FlowKit library as a companion to FlowIO.

## Installation

```bash
uv pip install flowio
```

Requires Python 3.9 or later.

## Quick Start

### Basic File Reading

```python
from flowio import FlowData

# Read FCS file
flow_data = FlowData('experiment.fcs')

# Access basic information
print(f"FCS Version: {flow_data.version}")
print(f"Events: {flow_data.event_count}")
print(f"Channels: {flow_data.pnn_labels}")

# Get event data as NumPy array
events = flow_data.as_array()  # Shape: (events, channels)
```

### Creating FCS Files

```python
import numpy as np
from flowio import create_fcs

# Prepare data
data = np.array([[100, 200, 50], [150, 180, 60]])  # 2 events, 3 channels
channels = ['FSC-A', 'SSC-A', 'FL1-A']

# Create FCS file
create_fcs('output.fcs', data, channels)
```

## Core Workflows

### Reading and Parsing FCS Files

The FlowData class provides the primary interface for reading FCS files.

**Standard Reading:**

```python
from flowio import FlowData

# Basic reading
flow = FlowData('sample.fcs')

# Access attributes
version = flow.version              # '3.0', '3.1', etc.
event_count = flow.event_count      # Number of events
channel_count = flow.channel_count  # Number of channels
pnn_labels = flow.pnn_labels        # Short channel names
pns_labels = flow.pns_labels        # Descriptive stain names

# Get event data
events = flow.as_array()            # Preprocessed (gain, log scaling applied)
raw_events = flow.as_array(preprocess=False)  # Raw data
```

**Memory-Efficient Metadata Reading:**

When only metadata is needed (no event data):

```python
# Only parse TEXT segment, skip DATA and ANALYSIS
flow = FlowData('sample.fcs', only_text=True)

# Access metadata
metadata = flow.text  # Dictionary of TEXT segment keywords
print(metadata.get('$DATE'))  # Acquisition date
print(metadata.get('$CYT'))   # Instrument name
```

**Handling Problematic Files:**

Some FCS files have offset discrepancies or errors:

```python
# Ignore offset discrepancies between HEADER and TEXT sections
flow = FlowData('problematic.f

... (truncated from original)
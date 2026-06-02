---
name: opentrons-integration
description: Official Opentrons Protocol API for OT-2 and Flex robots. Use when writing protocols specifically for Opentrons hardware with full access to Protocol API v2 features. Best for production Opentrons pro
triggers:
  - opentrons integration
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Opentrons Integration

## Overview

Opentrons is a Python-based lab automation platform for Flex and OT-2 robots. Write Protocol API v2 protocols for liquid handling, control hardware modules (heater-shaker, thermocycler), manage labware, for automated pipetting workflows.

## When to Use This Skill

This skill should be used when:
- Writing Opentrons Protocol API v2 protocols in Python
- Automating liquid handling workflows on Flex or OT-2 robots
- Controlling hardware modules (temperature, magnetic, heater-shaker, thermocycler)
- Setting up labware configurations and deck layouts
- Implementing complex pipetting operations (serial dilutions, plate replication, PCR setup)
- Managing tip usage and optimizing protocol efficiency
- Working with multi-channel pipettes for 96-well plate operations
- Simulating and testing protocols before robot execution

## Core Capabilities

### 1. Protocol Structure and Metadata

Every Opentrons protocol follows a standard structure:

```python
from opentrons import protocol_api

# Metadata
metadata = {
    'protocolName': 'My Protocol',
    'author': 'Name <email@example.com>',
    'description': 'Protocol description',
    'apiLevel': '2.19'  # Use latest available API version
}

# Requirements (optional)
requirements = {
    'robotType': 'Flex',  # or 'OT-2'
    'apiLevel': '2.19'
}

# Run function
def run(protocol: protocol_api.ProtocolContext):
    # Protocol commands go here
    pass
```

**Key elements:**
- Import `protocol_api` from `opentrons`
- Define `metadata` dict with protocolName, author, description, apiLevel
- Optional `requirements` dict for robot type and API version
- Implement `run()` function receiving `ProtocolContext` as parameter
- All protocol logic goes inside the `run()` function

### 2. Loading Hardware

**Loading Instruments (Pipettes):**

```python
def run(protocol: protocol_api.ProtocolContext):
    # Load pipette on specific mount
    left_pipette = protocol.load_instrument(
        'p1000_single_flex',  # Instrument name
        'left',               # Mount: 'left' or 'right'
        tip_racks=[tip_rack]  # List of tip rack labware objects
    )
```

Common pipette names:
- Flex: `p50_single_flex`, `p1000_single_flex`, `p50_multi_flex`, `p1000_multi_flex`
- OT-2: `p20_single_gen2`, `p300_single_gen2`, `p1000_single_gen2`, `p20_multi_gen2`, `p300_multi_gen2`

**Loading Labware:**

```python
# Load labware directly on deck
plate = protocol.load_labware(
    'corning_96_wellplate_360ul_flat',  # Labware API name
    'D1',                                # Deck slot (Flex: A1-D3, OT-2: 1-11)
    label='Sample Plate'                 # Optional display label
)

# Load tip rack
tip_rack = protocol.load_labware('opentrons_flex_96_tiprack_1000ul', 'C1')

# Load labware on adapter
adapter = protocol.load_adapter('opentrons_flex_96_tiprack_adapter', 'B1')
tips = adapter.load_labware('opentrons_flex_96_tiprack_200ul')
```

**Loading Modules:**

```python
# Temperature module
temp_module = p

... (truncated from original)
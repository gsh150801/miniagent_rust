---
name: pylabrobot
description: Vendor-agnostic lab automation framework. Use when controlling multiple equipment types (Hamilton, Tecan, Opentrons, plate readers, pumps) or needing unified programming across different vendors. Best
triggers:
  - pylabrobot
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# PyLabRobot

## Overview

PyLabRobot is a hardware-agnostic, pure Python Software Development Kit for automated and autonomous laboratories. Use this skill to control liquid handling robots, plate readers, pumps, heater shakers, incubators, centrifuges, and other laboratory automation equipment through a unified Python interface that works across platforms (Windows, macOS, Linux).

## When to Use This Skill

Use this skill when:
- Programming liquid handling robots (Hamilton STAR/STARlet, Opentrons OT-2, Tecan EVO)
- Automating laboratory workflows involving pipetting, sample preparation, or analytical measurements
- Managing deck layouts and laboratory resources (plates, tips, containers, troughs)
- Integrating multiple lab devices (liquid handlers, plate readers, heater shakers, pumps)
- Creating reproducible laboratory protocols with state management
- Simulating protocols before running on physical hardware
- Reading plates using BMG CLARIOstar or other supported plate readers
- Controlling temperature, shaking, centrifugation, or other material handling operations
- Working with laboratory automation in Python

## Core Capabilities

PyLabRobot provides comprehensive laboratory automation through six main capability areas, each detailed in the references/ directory:

### 1. Liquid Handling (`references/liquid-handling.md`)

Control liquid handling robots for aspirating, dispensing, and transferring liquids. Key operations include:
- **Basic Operations**: Aspirate, dispense, transfer liquids between wells
- **Tip Management**: Pick up, drop, and track pipette tips automatically
- **Advanced Techniques**: Multi-channel pipetting, serial dilutions, plate replication
- **Volume Tracking**: Automatic tracking of liquid volumes in wells
- **Hardware Support**: Hamilton STAR/STARlet, Opentrons OT-2, Tecan EVO, and others

### 2. Resource Management (`references/resources.md`)

Manage laboratory resources in a hierarchical system:
- **Resource Types**: Plates, tip racks, troughs, tubes, carriers, and custom labware
- **Deck Layout**: Assign resources to deck positions with coordinate systems
- **State Management**: Track tip presence, liquid volumes, and resource states
- **Serialization**: Save and load deck layouts and states from JSON files
- **Resource Discovery**: Access wells, tips, and containers through intuitive APIs

### 3. Hardware Backends (`references/hardware-backends.md`)

Connect to diverse laboratory equipment through backend abstraction:
- **Liquid Handlers**: Hamilton STAR (full support), Opentrons OT-2, Tecan EVO
- **Simulation**: ChatterboxBackend for protocol testing without hardware
- **Platform Support**: Works on Windows, macOS, Linux, and Raspberry Pi
- **Backend Switching**: Change robots by swapping backend without rewriting protocols

### 4. Analytical Equipment (`references/analytical-equipment.md`)

Integrate plate readers and analytical instruments:
- **Plate Readers**: BMG CLARIOstar for absorbance, luminescence, flu

... (truncated from original)
---
name: get-available-resources
description: This skill should be used at the start of any computationally intensive scientific task to detect and report available system resources (CPU cores, GPUs, memory, disk space). It creates a JSON file wi
triggers:
  - get available resources
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Get Available Resources

## Overview

Detect available computational resources and generate strategic recommendations for scientific computing tasks. This skill automatically identifies CPU capabilities, GPU availability (NVIDIA CUDA, AMD ROCm, Apple Silicon Metal), memory constraints, and disk space to help make informed decisions about computational approaches.

## When to Use This Skill

Use this skill proactively before any computationally intensive task:

- **Before data analysis**: Determine if datasets can be loaded into memory or require out-of-core processing
- **Before model training**: Check if GPU acceleration is available and which backend to use
- **Before parallel processing**: Identify optimal number of workers for joblib, multiprocessing, or Dask
- **Before large file operations**: Verify sufficient disk space and appropriate storage strategies
- **At project initialization**: Understand baseline capabilities for making architectural decisions

**Example scenarios:**
- "Help me analyze this 50GB genomics dataset" → Use this skill first to determine if Dask/Zarr are needed
- "Train a neural network on this data" → Use this skill to detect available GPUs and backends
- "Process 10,000 files in parallel" → Use this skill to determine optimal worker count
- "Run a computationally intensive simulation" → Use this skill to understand resource constraints

## How This Skill Works

### Resource Detection

The skill runs `scripts/detect_resources.py` to automatically detect:

1. **CPU Information**
   - Physical and logical core counts
   - Processor architecture and model
   - CPU frequency information

2. **GPU Information**
   - NVIDIA GPUs: Detects via nvidia-smi, reports VRAM, driver version, compute capability
   - AMD GPUs: Detects via rocm-smi
   - Apple Silicon: Detects M1/M2/M3/M4 chips with Metal support and unified memory

3. **Memory Information**
   - Total and available RAM
   - Current memory usage percentage
   - Swap space availability

4. **Disk Space Information**
   - Total and available disk space for working directory
   - Current usage percentage

5. **Operating System Information**
   - OS type (macOS, Linux, Windows)
   - OS version and release
   - Python version

### Output Format

The skill generates a `.claude_resources.json` file in the current working directory containing:

```json
{
  "timestamp": "2025-10-23T10:30:00",
  "os": {
    "system": "Darwin",
    "release": "25.0.0",
    "machine": "arm64"
  },
  "cpu": {
    "physical_cores": 8,
    "logical_cores": 8,
    "architecture": "arm64"
  },
  "memory": {
    "total_gb": 16.0,
    "available_gb": 8.5,
    "percent_used": 46.9
  },
  "disk": {
    "total_gb": 500.0,
    "available_gb": 200.0,
    "percent_used": 60.0
  },
  "gpu": {
    "nvidia_gpus": [],
    "amd_gpus": [],
    "apple_silicon": {
      "name": "Apple M2",
      "type": "Apple Silicon",
      "backend": "Metal",
      "unified_memory": true
    },
    "total_gpus": 1,
    "available_

... (truncated from original)
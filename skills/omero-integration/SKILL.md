---
name: omero-integration
description: Microscopy data management platform. Access images via Python, retrieve datasets, analyze pixels, manage ROIs/annotations, batch processing, for high-content screening and microscopy workflows.
triggers:
  - omero integration
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# OMERO Integration

## Overview

OMERO is an open-source platform for managing, visualizing, and analyzing microscopy images and metadata. Access images via Python API, retrieve datasets, analyze pixels, manage ROIs and annotations, for high-content screening and microscopy workflows.

## When to Use This Skill

This skill should be used when:
- Working with OMERO Python API (omero-py) to access microscopy data
- Retrieving images, datasets, projects, or screening data programmatically
- Analyzing pixel data and creating derived images
- Creating or managing ROIs (regions of interest) on microscopy images
- Adding annotations, tags, or metadata to OMERO objects
- Storing measurement results in OMERO tables
- Creating server-side scripts for batch processing
- Performing high-content screening analysis

## Core Capabilities

This skill covers eight major capability areas. Each is documented in detail in the references/ directory:

### 1. Connection & Session Management
**File**: `references/connection.md`

Establish secure connections to OMERO servers, manage sessions, handle authentication, and work with group contexts. Use this for initial setup and connection patterns.

**Common scenarios:**
- Connect to OMERO server with credentials
- Use existing session IDs
- Switch between group contexts
- Manage connection lifecycle with context managers

### 2. Data Access & Retrieval
**File**: `references/data_access.md`

Navigate OMERO's hierarchical data structure (Projects → Datasets → Images) and screening data (Screens → Plates → Wells). Retrieve objects, query by attributes, and access metadata.

**Common scenarios:**
- List all projects and datasets for a user
- Retrieve images by ID or dataset
- Access screening plate data
- Query objects with filters

### 3. Metadata & Annotations
**File**: `references/metadata.md`

Create and manage annotations including tags, key-value pairs, file attachments, and comments. Link annotations to images, datasets, or other objects.

**Common scenarios:**
- Add tags to images
- Attach analysis results as files
- Create custom key-value metadata
- Query annotations by namespace

### 4. Image Processing & Rendering
**File**: `references/image_processing.md`

Access raw pixel data as NumPy arrays, manipulate rendering settings, create derived images, and manage physical dimensions.

**Common scenarios:**
- Extract pixel data for computational analysis
- Generate thumbnail images
- Create maximum intensity projections
- Modify channel rendering settings

### 5. Regions of Interest (ROIs)
**File**: `references/rois.md`

Create, retrieve, and analyze ROIs with various shapes (rectangles, ellipses, polygons, masks, points, lines). Extract intensity statistics from ROI regions.

**Common scenarios:**
- Draw rectangular ROIs on images
- Create polygon masks for segmentation
- Analyze pixel intensities within ROIs
- Export ROI coordinates

### 6. OMERO Tables
**File**: `references/tables.md`

Store and query structured tab

... (truncated from original)
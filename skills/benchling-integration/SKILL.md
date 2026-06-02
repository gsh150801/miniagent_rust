---
name: benchling-integration
description: Benchling R&D platform integration. Access registry (DNA, proteins), inventory, ELN entries, workflows via API, build Benchling Apps, query Data Warehouse, for lab data management automation.
triggers:
  - benchling integration
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Benchling Integration

## Overview

Benchling is a cloud platform for life sciences R&D. Access registry entities (DNA, proteins), inventory, electronic lab notebooks, and workflows programmatically via Python SDK and REST API.

## When to Use This Skill

This skill should be used when:
- Working with Benchling's Python SDK or REST API
- Managing biological sequences (DNA, RNA, proteins) and registry entities
- Automating inventory operations (samples, containers, locations, transfers)
- Creating or querying electronic lab notebook entries
- Building workflow automations or Benchling Apps
- Syncing data between Benchling and external systems
- Querying the Benchling Data Warehouse for analytics
- Setting up event-driven integrations with AWS EventBridge

## Core Capabilities

### 1. Authentication & Setup

**Python SDK Installation:**
```python
# Stable release
uv pip install benchling-sdk
# or with Poetry
poetry add benchling-sdk
```

**Authentication Methods:**

API Key Authentication (recommended for scripts):
```python
from benchling_sdk.benchling import Benchling
from benchling_sdk.auth.api_key_auth import ApiKeyAuth

benchling = Benchling(
    url="https://your-tenant.benchling.com",
    auth_method=ApiKeyAuth("your_api_key")
)
```

OAuth Client Credentials (for apps):
```python
from benchling_sdk.auth.client_credentials_oauth2 import ClientCredentialsOAuth2

auth_method = ClientCredentialsOAuth2(
    client_id="your_client_id",
    client_secret="your_client_secret"
)
benchling = Benchling(
    url="https://your-tenant.benchling.com",
    auth_method=auth_method
)
```

**Key Points:**
- API keys are obtained from Profile Settings in Benchling
- Store credentials securely (use environment variables or password managers)
- All API requests require HTTPS
- Authentication permissions mirror user permissions in the UI

For detailed authentication information including OIDC and security best practices, refer to `references/authentication.md`.

### 2. Registry & Entity Management

Registry entities include DNA sequences, RNA sequences, AA sequences, custom entities, and mixtures. The SDK provides typed classes for creating and managing these entities.

**Creating DNA Sequences:**
```python
from benchling_sdk.models import DnaSequenceCreate

sequence = benchling.dna_sequences.create(
    DnaSequenceCreate(
        name="My Plasmid",
        bases="ATCGATCG",
        is_circular=True,
        folder_id="fld_abc123",
        schema_id="ts_abc123",  # optional
        fields=benchling.models.fields({"gene_name": "GFP"})
    )
)
```

**Registry Registration:**

To register an entity directly upon creation:
```python
sequence = benchling.dna_sequences.create(
    DnaSequenceCreate(
        name="My Plasmid",
        bases="ATCGATCG",
        is_circular=True,
        folder_id="fld_abc123",
        entity_registry_id="src_abc123",  # Registry to register in
        naming_strategy="NEW_IDS"  # or "IDS_FROM_NAMES"
    )
)
```

**Important:** Use eithe

... (truncated from original)
---
name: open-notebook
description: Self-hosted, open-source alternative to Google NotebookLM for AI-powered research and document analysis. Use when organizing research materials into notebooks, ingesting diverse content sources (PDFs,
triggers:
  - open notebook
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Open Notebook

## Overview

Open Notebook is an open-source, self-hosted alternative to Google's NotebookLM that enables researchers to organize materials, generate AI-powered insights, create podcasts, and have context-aware conversations with their documents — all while maintaining complete data privacy.

Unlike Google's Notebook LM, which has no publicly available API outside of the Enterprise version, Open Notebook provides a comprehensive REST API, supports 16+ AI providers, and runs entirely on your own infrastructure.

**Key advantages over NotebookLM:**
- Full REST API for programmatic access and automation
- Choice of 16+ AI providers (not locked to Google models)
- Multi-speaker podcast generation with 1-4 customizable speakers (vs. 2-speaker limit)
- Complete data sovereignty through self-hosting
- Open source and fully extensible (MIT license)

**Repository:** https://github.com/lfnovo/open-notebook

## Quick Start

### Prerequisites

- Docker Desktop installed
- API key for at least one AI provider (or local Ollama for free local inference)

### Installation

Deploy Open Notebook using Docker Compose:

```bash
# Download the docker-compose file
curl -o docker-compose.yml https://raw.githubusercontent.com/lfnovo/open-notebook/main/docker-compose.yml

# Set the required encryption key
export OPEN_NOTEBOOK_ENCRYPTION_KEY="your-secret-key-here"

# Launch the services
docker-compose up -d
```

Access the application:
- **Frontend UI:** http://localhost:8502
- **REST API:** http://localhost:5055
- **API Documentation:** http://localhost:5055/docs

### Configure AI Provider

After startup, configure at least one AI provider:

1. Navigate to **Settings > API Keys** in the UI
2. Add credentials for your preferred provider (OpenAI, Anthropic, etc.)
3. Test the connection and discover available models
4. Register models for use across the platform

Or configure via the REST API:

```python
import requests

BASE_URL = "http://localhost:5055/api"

# Add a credential for an AI provider
response = requests.post(f"{BASE_URL}/credentials", json={
    "provider": "openai",
    "name": "My OpenAI Key",
    "api_key": "sk-..."
})
credential = response.json()

# Discover available models
response = requests.post(
    f"{BASE_URL}/credentials/{credential['id']}/discover"
)
discovered = response.json()

# Register discovered models
requests.post(
    f"{BASE_URL}/credentials/{credential['id']}/register-models",
    json={"model_ids": [m["id"] for m in discovered["models"]]}
)
```

## Core Features

### Notebooks
Organize research into separate notebooks, each containing sources, notes, and chat sessions.

```python
import requests

BASE_URL = "http://localhost:5055/api"

# Create a notebook
response = requests.post(f"{BASE_URL}/notebooks", json={
    "name": "Cancer Genomics Research",
    "description": "Literature review on tumor mutational burden"
})
notebook = response.json()
notebook_id = notebook["id"]
```

### Sources
Ingest diverse content types 

... (truncated from original)
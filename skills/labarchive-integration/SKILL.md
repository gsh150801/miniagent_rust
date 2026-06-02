---
name: labarchive-integration
description: Electronic lab notebook API integration. Access notebooks, manage entries/attachments, backup notebooks, integrate with Protocols.io/Jupyter/REDCap, for programmatic ELN workflows.
triggers:
  - labarchive integration
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# LabArchives Integration

## Overview

LabArchives is an electronic lab notebook platform for research documentation and data management. Access notebooks, manage entries and attachments, generate reports, and integrate with third-party tools programmatically via REST API.

## When to Use This Skill

This skill should be used when:
- Working with LabArchives REST API for notebook automation
- Backing up notebooks programmatically
- Creating or managing notebook entries and attachments
- Generating site reports and analytics
- Integrating LabArchives with third-party tools (Protocols.io, Jupyter, REDCap)
- Automating data upload to electronic lab notebooks
- Managing user access and permissions programmatically

## Core Capabilities

### 1. Authentication and Configuration

Set up API access credentials and regional endpoints for LabArchives API integration.

**Prerequisites:**
- Enterprise LabArchives license with API access enabled
- API access key ID and password from LabArchives administrator
- User authentication credentials (email and external applications password)

**Configuration setup:**

Use the `scripts/setup_config.py` script to create a configuration file:

```bash
python3 scripts/setup_config.py
```

This creates a `config.yaml` file with the following structure:

```yaml
api_url: https://api.labarchives.com/api  # or regional endpoint
access_key_id: YOUR_ACCESS_KEY_ID
access_password: YOUR_ACCESS_PASSWORD
```

**Regional API endpoints:**
- US/International: `https://api.labarchives.com/api`
- Australia: `https://auapi.labarchives.com/api`
- UK: `https://ukapi.labarchives.com/api`

For detailed authentication instructions and troubleshooting, refer to `references/authentication_guide.md`.

### 2. User Information Retrieval

Obtain user ID (UID) and access information required for subsequent API operations.

**Workflow:**

1. Call the `users/user_access_info` API method with login credentials
2. Parse the XML/JSON response to extract the user ID (UID)
3. Use the UID to retrieve detailed user information via `users/user_info_via_id`

**Example using Python wrapper:**

```python
from labarchivespy.client import Client

# Initialize client
client = Client(api_url, access_key_id, access_password)

# Get user access info
login_params = {'login_or_email': user_email, 'password': auth_token}
response = client.make_call('users', 'user_access_info', params=login_params)

# Extract UID from response
import xml.etree.ElementTree as ET
uid = ET.fromstring(response.content)[0].text

# Get detailed user info
params = {'uid': uid}
user_info = client.make_call('users', 'user_info_via_id', params=params)
```

### 3. Notebook Operations

Manage notebook access, backup, and metadata retrieval.

**Key operations:**

- **List notebooks:** Retrieve all notebooks accessible to a user
- **Backup notebooks:** Download complete notebook data with optional attachment inclusion
- **Get notebook IDs:** Retrieve institution-defined notebook identifiers for integ

... (truncated from original)
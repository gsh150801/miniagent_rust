---
name: pyzotero
description: Interact with Zotero reference management libraries using the pyzotero Python client. Retrieve, create, update, and delete items, collections, tags, and attachments via the Zotero Web API v3. Use this
triggers:
  - pyzotero
tools_needed:
  - read
  - write
  - edit
  - bash
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Pyzotero

Pyzotero is a Python wrapper for the [Zotero API v3](https://www.zotero.org/support/dev/web_api/v3/start). Use it to programmatically manage Zotero libraries: read items and collections, create and update references, upload attachments, manage tags, and export citations.

## Authentication Setup

**Required credentials** — get from https://www.zotero.org/settings/keys:
- **User ID**: shown as "Your userID for use in API calls"
- **API Key**: create at https://www.zotero.org/settings/keys/new
- **Library ID**: for group libraries, the integer after `/groups/` in the group URL

Store credentials in environment variables or a `.env` file:
```
ZOTERO_LIBRARY_ID=your_user_id
ZOTERO_API_KEY=your_api_key
ZOTERO_LIBRARY_TYPE=user  # or "group"
```

See [references/authentication.md](references/authentication.md) for full setup details.

## Installation

```bash
uv add pyzotero
# or with CLI support:
uv add "pyzotero[cli]"
```

## Quick Start

```python
from pyzotero import Zotero

zot = Zotero(library_id='123456', library_type='user', api_key='ABC1234XYZ')

# Retrieve top-level items (returns 100 by default)
items = zot.top(limit=10)
for item in items:
    print(item['data']['title'], item['data']['itemType'])

# Search by keyword
results = zot.items(q='machine learning', limit=20)

# Retrieve all items (use everything() for complete results)
all_items = zot.everything(zot.items())
```

## Core Concepts

- A `Zotero` instance is bound to a single library (user or group). All methods operate on that library.
- Item data lives in `item['data']`. Access fields like `item['data']['title']`, `item['data']['creators']`.
- Pyzotero returns 100 items by default (API default is 25). Use `zot.everything(zot.items())` to get all items.
- Write methods return `True` on success or raise a `ZoteroError`.

## Reference Files

| File | Contents |
|------|----------|
| [references/authentication.md](references/authentication.md) | Credentials, library types, local mode |
| [references/read-api.md](references/read-api.md) | Retrieving items, collections, tags, groups |
| [references/search-params.md](references/search-params.md) | Filtering, sorting, search parameters |
| [references/write-api.md](references/write-api.md) | Creating, updating, deleting items |
| [references/collections.md](references/collections.md) | Collection CRUD operations |
| [references/tags.md](references/tags.md) | Tag retrieval and management |
| [references/files-attachments.md](references/files-attachments.md) | File retrieval and attachment uploads |
| [references/exports.md](references/exports.md) | BibTeX, CSL-JSON, bibliography export |
| [references/pagination.md](references/pagination.md) | follow(), everything(), generators |
| [references/full-text.md](references/full-text.md) | Full-text content indexing and retrieval |
| [references/saved-searches.md](references/saved-searches.md) | Saved search management |
| [references/cli.md](references/cli.md) | Command-line interfac

... (truncated from original)
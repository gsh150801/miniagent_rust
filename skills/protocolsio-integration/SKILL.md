---
name: protocolsio-integration
description: Integration with protocols.io API for managing scientific protocols. This skill should be used when working with protocols.io to search, create, update, or publish protocols; manage protocol steps and
triggers:
  - protocolsio integration
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Protocols.io Integration

## Overview

Protocols.io is a comprehensive platform for developing, sharing, and managing scientific protocols. This skill provides complete integration with the protocols.io API v3, enabling programmatic access to protocols, workspaces, discussions, file management, and collaboration features.

## When to Use This Skill

Use this skill when working with protocols.io in any of the following scenarios:

- **Protocol Discovery**: Searching for existing protocols by keywords, DOI, or category
- **Protocol Management**: Creating, updating, or publishing scientific protocols
- **Step Management**: Adding, editing, or organizing protocol steps and procedures
- **Collaborative Development**: Working with team members on shared protocols
- **Workspace Organization**: Managing lab or institutional protocol repositories
- **Discussion & Feedback**: Adding or responding to protocol comments
- **File Management**: Uploading data files, images, or documents to protocols
- **Experiment Tracking**: Documenting protocol executions and results
- **Data Export**: Backing up or migrating protocol collections
- **Integration Projects**: Building tools that interact with protocols.io

## Core Capabilities

This skill provides comprehensive guidance across five major capability areas:

### 1. Authentication & Access

Manage API authentication using access tokens and OAuth flows. Includes both client access tokens (for personal content) and OAuth tokens (for multi-user applications).

**Key operations:**
- Generate authorization links for OAuth flow
- Exchange authorization codes for access tokens
- Refresh expired tokens
- Manage rate limits and permissions

**Reference:** Read `references/authentication.md` for detailed authentication procedures, OAuth implementation, and security best practices.

### 2. Protocol Operations

Complete protocol lifecycle management from creation to publication.

**Key operations:**
- Search and discover protocols by keywords, filters, or DOI
- Retrieve detailed protocol information with all steps
- Create new protocols with metadata and tags
- Update protocol information and settings
- Manage protocol steps (create, update, delete, reorder)
- Handle protocol materials and reagents
- Publish protocols with DOI issuance
- Bookmark protocols for quick access
- Generate protocol PDFs

**Reference:** Read `references/protocols_api.md` for comprehensive protocol management guidance, including API endpoints, parameters, common workflows, and examples.

### 3. Discussions & Collaboration

Enable community engagement through comments and discussions.

**Key operations:**
- View protocol-level and step-level comments
- Create new comments and threaded replies
- Edit or delete your own comments
- Analyze discussion patterns and feedback
- Respond to user questions and issues

**Reference:** Read `references/discussions.md` for discussion management, comment threading, and collaboration workflows.

### 4. Workspace Ma

... (truncated from original)
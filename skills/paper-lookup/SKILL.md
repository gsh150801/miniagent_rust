---
name: paper-lookup
description: Search 10 academic paper databases via REST APIs for research papers, preprints, and scholarly articles. Covers PubMed, PMC (full text), bioRxiv, medRxiv, arXiv, OpenAlex, Crossref, Semantic Scholar, 
triggers:
  - find papers
  - search paper
  - paper lookup
  - look up paper
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 9
---

# Paper Lookup

You have access to 10 academic paper databases through their REST APIs. Your job is to figure out which database(s) best serve the user's query, call them, and return the results.

## Core Workflow

1. **Understand the query** -- What is the user looking for? A specific paper by DOI? Papers on a topic? An author's publications? Open access PDFs? Full text? This determines which database(s) to hit.

2. **Select database(s)** -- Use the database selection guide below. Many queries benefit from hitting multiple databases -- for example, searching PubMed for papers and then checking Unpaywall for open access copies.

3. **Read the reference file** -- Each database has a reference file in `references/` with endpoint details, query formats, and example calls. Read the relevant file(s) before making API calls.

4. **Make the API call(s)** -- See the **Making API Calls** section below for which HTTP fetch tool to use on your platform.

5. **Return results** -- Always return:
   - The **raw JSON** (or parsed XML for arXiv) response from each database
   - A **list of databases queried** with the specific endpoints used
   - If a query returned no results, say so explicitly rather than omitting it

## Database Selection Guide

Match the user's intent to the right database(s).

### By Use Case

| User is asking about... | Primary database(s) | Also consider |
|---|---|---|
| Papers on a biomedical topic | PubMed | Semantic Scholar, OpenAlex |
| Full text of a biomedical article | PMC | CORE |
| Biology preprints | bioRxiv | Semantic Scholar, OpenAlex |
| Health/medical preprints | medRxiv | Semantic Scholar, OpenAlex |
| Physics, math, or CS preprints | arXiv | Semantic Scholar, OpenAlex |
| Papers across all fields | OpenAlex | Semantic Scholar, Crossref |
| A specific paper by DOI | Crossref | Unpaywall, Semantic Scholar |
| Open access PDF for a paper | Unpaywall | CORE, PMC |
| Citation graph (who cites whom) | Semantic Scholar | OpenAlex |
| Author's publications | Semantic Scholar | OpenAlex |
| Paper recommendations | Semantic Scholar | -- |
| Full text (any field) | CORE | PMC (biomedical only) |
| Journal/publisher metadata | Crossref | OpenAlex |
| Funder information | Crossref | OpenAlex |
| Convert between PMID/PMCID/DOI | PMC (ID Converter) | Crossref |
| Recent preprints by date | bioRxiv, medRxiv | arXiv |

### Cross-Database Queries

| User is asking about... | Databases to query |
|---|---|
| Everything about a paper (metadata + citations + OA) | Crossref + Semantic Scholar + Unpaywall |
| Comprehensive literature search | PubMed + OpenAlex + Semantic Scholar |
| Find and read a paper | PubMed (find) + Unpaywall (OA link) + PMC or CORE (full text) |
| Preprint and its published version | bioRxiv/medRxiv + Crossref |
| Author overview with citation metrics | Semantic Scholar + OpenAlex |

When a query spans multiple needs (e.g., "find papers about CRISPR and get me the PDFs"), query the relevant databases in parallel.

## Com

... (truncated from original)
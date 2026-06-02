---
name: hugging-science
description: Use when the user is doing AI/ML work in a scientific domain — biology, chemistry, physics, astronomy, climate, genomics, materials science, medicine, ecology, energy, conservation, engineering, mathe
triggers:
  - hugging science
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Hugging Science

Hugging Science is a curated, LLM-friendly index of scientific datasets, models, blog posts, and interactive demos for ML researchers. Use it when a scientific ML question lands in front of you — it's much higher signal than generic search and the entries are pre-filtered for quality and openness.

There are two related surfaces, and you should use both:

- **The catalog at `huggingscience.co`** — a static, parseable index of resources across 17 scientific domains. It exposes `llms.txt` (compact), `llms-full.txt` (full content), and `topics/<slug>.md` (per-domain). These are markdown files designed to be fetched and read.
- **The `hugging-science` Hugging Face organization** — `huggingface.co/hugging-science` — community-submitted datasets, a few models, and ~27 interactive Spaces (notably BoltzGen for protein/binder design, Dataset Quest for submissions, and Science Release Heatmap for ecosystem visualization).

The catalog *points to* resources hosted on the broader Hugging Face Hub. So an entry like `arcinstitute/opengenome2` is a regular HF dataset that you load with the `datasets` library; an entry like `facebook/esm2_t33_650M_UR50D` is a regular HF model you load with `transformers`. The catalog's job is curation and discovery; usage goes through standard Hugging Face APIs.

## When to use this skill

Engage this skill when the user's task involves AI/ML applied to science. Common signals:

- Names a scientific domain (protein, genome, molecule, crystal, weather, climate, galaxy, EEG, microbiome, pathology, plasma, …)
- Asks "is there a dataset/model for X" where X is scientific
- Wants to fine-tune on scientific data, evaluate on scientific benchmarks, or reproduce a scientific ML paper
- Asks about specific known scientific models (Evo-2, ESM2, BoltzGen, Nucleotide Transformer, AlphaFold-derived, etc.)
- Needs an interactive demo for a scientific task (binder design, theorem proving, etc.)

If the task is generic ML (recommendation systems, chatbot RAG, vision on cats and dogs), this skill is **not** the right tool — defer to general HF Hub knowledge instead.

## Core workflow

Most invocations follow this five-step loop. Don't skip discovery — the value of Hugging Science is that it has already filtered hundreds of resources down to high-signal picks per domain.

### 1. Identify the domain(s)

Map the user's task to one or more of the 17 topic slugs:

`astronomy` · `benchmark` · `biology` · `biotechnology` · `chemistry` · `climate` · `conservation` · `earth-science` · `ecology` · `energy` · `engineering` · `genomics` · `materials-science` · `mathematics` · `medicine` · `physics` · `scientific-reasoning`

Some tasks span multiple topics (e.g., drug discovery → `chemistry` + `biology` + `medicine`). Fetch each relevant topic.

### 2. Fetch the relevant catalog content

Use the bundled script for clean, structured access:

```bash
python scripts/fetch_catalog.py topic biology
python scripts/fetch_catalog.py topic material

... (truncated from original)
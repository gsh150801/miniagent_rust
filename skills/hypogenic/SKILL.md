---
name: hypogenic
description: Automated LLM-driven hypothesis generation and testing on tabular datasets. Use when you want to systematically explore hypotheses about patterns in empirical data (e.g., deception detection, content 
triggers:
  - hypogenic
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Hypogenic

## Overview

Hypogenic provides automated hypothesis generation and testing using large language models to accelerate scientific discovery. The framework supports three approaches: HypoGeniC (data-driven hypothesis generation), HypoRefine (synergistic literature and data integration), and Union methods (mechanistic combination of literature and data-driven hypotheses).

## Quick Start

Get started with Hypogenic in minutes:

```bash
# Install the package
uv pip install hypogenic

# Clone example datasets
git clone https://github.com/ChicagoHAI/HypoGeniC-datasets.git ./data

# Run basic hypothesis generation
hypogenic_generation --config ./data/your_task/config.yaml --method hypogenic --num_hypotheses 20

# Run inference on generated hypotheses
hypogenic_inference --config ./data/your_task/config.yaml --hypotheses output/hypotheses.json
```

**Or use Python API:**

```python
from hypogenic import BaseTask

# Create task with your configuration
task = BaseTask(config_path="./data/your_task/config.yaml")

# Generate hypotheses
task.generate_hypotheses(method="hypogenic", num_hypotheses=20)

# Run inference
results = task.inference(hypothesis_bank="./output/hypotheses.json")
```

## When to Use This Skill

Use this skill when working on:
- Generating scientific hypotheses from observational datasets
- Testing multiple competing hypotheses systematically
- Combining literature insights with empirical patterns
- Accelerating research discovery through automated hypothesis ideation
- Domains requiring hypothesis-driven analysis: deception detection, AI-generated content identification, mental health indicators, predictive modeling, or other empirical research

## Key Features

**Automated Hypothesis Generation**
- Generate 10-20+ testable hypotheses from data in minutes
- Iterative refinement based on validation performance
- Support for both API-based (OpenAI, Anthropic) and local LLMs

**Literature Integration**
- Extract insights from research papers via PDF processing
- Combine theoretical foundations with empirical patterns
- Systematic literature-to-hypothesis pipeline with GROBID

**Performance Optimization**
- Redis caching reduces API costs for repeated experiments
- Parallel processing for large-scale hypothesis testing
- Adaptive refinement focuses on challenging examples

**Flexible Configuration**
- Template-based prompt engineering with variable injection
- Custom label extraction for domain-specific tasks
- Modular architecture for easy extension

**Proven Results**
- 8.97% improvement over few-shot baselines
- 15.75% improvement over literature-only approaches
- 80-84% hypothesis diversity (non-redundant insights)
- Human evaluators report significant decision-making improvements

## Core Capabilities

### 1. HypoGeniC: Data-Driven Hypothesis Generation

Generate hypotheses solely from observational data through iterative refinement.

**Process:**
1. Initialize with a small data subset to generate candidate hypotheses
2. Ite

... (truncated from original)
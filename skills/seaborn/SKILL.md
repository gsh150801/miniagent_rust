---
name: seaborn
description: Statistical visualization with pandas integration. Use for quick exploration of distributions, relationships, and categorical comparisons with attractive defaults. Best for box plots, violin plots, pa
triggers:
  - seaborn
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 6
---

# Seaborn Statistical Visualization

## Overview

Seaborn is a Python visualization library for creating publication-quality statistical graphics. Use this skill for dataset-oriented plotting, multivariate analysis, automatic statistical estimation, and complex multi-panel figures with minimal code.

## Design Philosophy

Seaborn follows these core principles:

1. **Dataset-oriented**: Work directly with DataFrames and named variables rather than abstract coordinates
2. **Semantic mapping**: Automatically translate data values into visual properties (colors, sizes, styles)
3. **Statistical awareness**: Built-in aggregation, error estimation, and confidence intervals
4. **Aesthetic defaults**: Publication-ready themes and color palettes out of the box
5. **Matplotlib integration**: Full compatibility with matplotlib customization when needed

## Quick Start

```python
import seaborn as sns
import matplotlib.pyplot as plt
import pandas as pd

# Load example dataset
df = sns.load_dataset('tips')

# Create a simple visualization
sns.scatterplot(data=df, x='total_bill', y='tip', hue='day')
plt.show()
```

## Core Plotting Interfaces

### Function Interface (Traditional)

The function interface provides specialized plotting functions organized by visualization type. Each category has **axes-level** functions (plot to single axes) and **figure-level** functions (manage entire figure with faceting).

**When to use:**
- Quick exploratory analysis
- Single-purpose visualizations
- When you need a specific plot type

### Objects Interface (Modern)

The `seaborn.objects` interface provides a declarative, composable API similar to ggplot2. Build visualizations by chaining methods to specify data mappings, marks, transformations, and scales.

**When to use:**
- Complex layered visualizations
- When you need fine-grained control over transformations
- Building custom plot types
- Programmatic plot generation

```python
from seaborn import objects as so

# Declarative syntax
(
    so.Plot(data=df, x='total_bill', y='tip')
    .add(so.Dot(), color='day')
    .add(so.Line(), so.PolyFit())
)
```

## Plotting Functions by Category

### Relational Plots (Relationships Between Variables)

**Use for:** Exploring how two or more variables relate to each other

- `scatterplot()` - Display individual observations as points
- `lineplot()` - Show trends and changes (automatically aggregates and computes CI)
- `relplot()` - Figure-level interface with automatic faceting

**Key parameters:**
- `x`, `y` - Primary variables
- `hue` - Color encoding for additional categorical/continuous variable
- `size` - Point/line size encoding
- `style` - Marker/line style encoding
- `col`, `row` - Facet into multiple subplots (figure-level only)

```python
# Scatter with multiple semantic mappings
sns.scatterplot(data=df, x='total_bill', y='tip',
                hue='time', size='size', style='sex')

# Line plot with confidence intervals
sns.lineplot(data=timeseries, x='date', y='value', hu

... (truncated from original)
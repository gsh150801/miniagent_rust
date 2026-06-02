---
name: pymoo
description: Multi-objective optimization framework. NSGA-II, NSGA-III, MOEA/D, Pareto fronts, constraint handling, benchmarks (ZDT, DTLZ), for engineering design and optimization problems.
triggers:
  - pymoo
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Pymoo - Multi-Objective Optimization in Python

## Overview

Pymoo is a comprehensive Python framework for optimization with emphasis on multi-objective problems. Solve single and multi-objective optimization using state-of-the-art algorithms (NSGA-II/III, MOEA/D), benchmark problems (ZDT, DTLZ), customizable genetic operators, and multi-criteria decision making methods. Excels at finding trade-off solutions (Pareto fronts) for problems with conflicting objectives.

## When to Use This Skill

This skill should be used when:
- Solving optimization problems with one or multiple objectives
- Finding Pareto-optimal solutions and analyzing trade-offs
- Implementing evolutionary algorithms (GA, DE, PSO, NSGA-II/III)
- Working with constrained optimization problems
- Benchmarking algorithms on standard test problems (ZDT, DTLZ, WFG)
- Customizing genetic operators (crossover, mutation, selection)
- Visualizing high-dimensional optimization results
- Making decisions from multiple competing solutions
- Handling binary, discrete, continuous, or mixed-variable problems

## Core Concepts

### The Unified Interface

Pymoo uses a consistent `minimize()` function for all optimization tasks:

```python
from pymoo.optimize import minimize

result = minimize(
    problem,        # What to optimize
    algorithm,      # How to optimize
    termination,    # When to stop
    seed=1,
    verbose=True
)
```

**Result object contains:**
- `result.X`: Decision variables of optimal solution(s)
- `result.F`: Objective values of optimal solution(s)
- `result.G`: Constraint violations (if constrained)
- `result.algorithm`: Algorithm object with history

### Problem Types

**Single-objective:** One objective to minimize/maximize
**Multi-objective:** 2-3 conflicting objectives → Pareto front
**Many-objective:** 4+ objectives → High-dimensional Pareto front
**Constrained:** Objectives + inequality/equality constraints
**Dynamic:** Time-varying objectives or constraints

## Quick Start Workflows

### Workflow 1: Single-Objective Optimization

**When:** Optimizing one objective function

**Steps:**
1. Define or select problem
2. Choose single-objective algorithm (GA, DE, PSO, CMA-ES)
3. Configure termination criteria
4. Run optimization
5. Extract best solution

**Example:**
```python
from pymoo.algorithms.soo.nonconvex.ga import GA
from pymoo.problems import get_problem
from pymoo.optimize import minimize

# Built-in problem
problem = get_problem("rastrigin", n_var=10)

# Configure Genetic Algorithm
algorithm = GA(
    pop_size=100,
    eliminate_duplicates=True
)

# Optimize
result = minimize(
    problem,
    algorithm,
    ('n_gen', 200),
    seed=1,
    verbose=True
)

print(f"Best solution: {result.X}")
print(f"Best objective: {result.F[0]}")
```

**See:** `scripts/single_objective_example.py` for complete example

### Workflow 2: Multi-Objective Optimization (2-3 objectives)

**When:** Optimizing 2-3 conflicting objectives, need Pareto front

**Algorithm choice:** NSGA

... (truncated from original)
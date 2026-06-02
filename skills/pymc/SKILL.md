---
name: pymc
description: Bayesian modeling with PyMC. Build hierarchical models, MCMC (NUTS), variational inference, LOO/WAIC comparison, posterior checks, for probabilistic programming and inference.
triggers:
  - pymc
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 7
---

# PyMC Bayesian Modeling

## Overview

PyMC is a Python library for Bayesian modeling and probabilistic programming. Build, fit, validate, and compare Bayesian models using PyMC's modern API (version 5.x+), including hierarchical models, MCMC sampling (NUTS), variational inference, and model comparison (LOO, WAIC).

## When to Use This Skill

This skill should be used when:
- Building Bayesian models (linear/logistic regression, hierarchical models, time series, etc.)
- Performing MCMC sampling or variational inference
- Conducting prior/posterior predictive checks
- Diagnosing sampling issues (divergences, convergence, ESS)
- Comparing multiple models using information criteria (LOO, WAIC)
- Implementing uncertainty quantification through Bayesian methods
- Working with hierarchical/multilevel data structures
- Handling missing data or measurement error in a principled way

## Standard Bayesian Workflow

Follow this workflow for building and validating Bayesian models:

### 1. Data Preparation

```python
import pymc as pm
import arviz as az
import numpy as np

# Load and prepare data
X = ...  # Predictors
y = ...  # Outcomes

# Standardize predictors for better sampling
X_mean = X.mean(axis=0)
X_std = X.std(axis=0)
X_scaled = (X - X_mean) / X_std
```

**Key practices:**
- Standardize continuous predictors (improves sampling efficiency)
- Center outcomes when possible
- Handle missing data explicitly (treat as parameters)
- Use named dimensions with `coords` for clarity

### 2. Model Building

```python
coords = {
    'predictors': ['var1', 'var2', 'var3'],
    'obs_id': np.arange(len(y))
}

with pm.Model(coords=coords) as model:
    # Priors
    alpha = pm.Normal('alpha', mu=0, sigma=1)
    beta = pm.Normal('beta', mu=0, sigma=1, dims='predictors')
    sigma = pm.HalfNormal('sigma', sigma=1)

    # Linear predictor
    mu = alpha + pm.math.dot(X_scaled, beta)

    # Likelihood
    y_obs = pm.Normal('y_obs', mu=mu, sigma=sigma, observed=y, dims='obs_id')
```

**Key practices:**
- Use weakly informative priors (not flat priors)
- Use `HalfNormal` or `Exponential` for scale parameters
- Use named dimensions (`dims`) instead of `shape` when possible
- Use `pm.Data()` for values that will be updated for predictions

### 3. Prior Predictive Check

**Always validate priors before fitting:**

```python
with model:
    prior_pred = pm.sample_prior_predictive(samples=1000, random_seed=42)

# Visualize
az.plot_ppc(prior_pred, group='prior')
```

**Check:**
- Do prior predictions span reasonable values?
- Are extreme values plausible given domain knowledge?
- If priors generate implausible data, adjust and re-check

### 4. Fit Model

```python
with model:
    # Optional: Quick exploration with ADVI
    # approx = pm.fit(n=20000)

    # Full MCMC inference
    idata = pm.sample(
        draws=2000,
        tune=1000,
        chains=4,
        target_accept=0.9,
        random_seed=42,
        idata_kwargs={'log_likelihood': True}  # For model comparison
    

... (truncated from original)
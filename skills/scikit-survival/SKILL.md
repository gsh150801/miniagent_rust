---
name: scikit-survival
description: Comprehensive toolkit for survival analysis and time-to-event modeling in Python using scikit-survival. Use this skill when working with censored survival data, performing time-to-event analysis, fitt
triggers:
  - scikit survival
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# scikit-survival: Survival Analysis in Python

## Overview

scikit-survival is a Python library for survival analysis built on top of scikit-learn. It provides specialized tools for time-to-event analysis, handling the unique challenge of censored data where some observations are only partially known.

Survival analysis aims to establish connections between covariates and the time of an event, accounting for censored records (particularly right-censored data from studies where participants don't experience events during observation periods).

## When to Use This Skill

Use this skill when:
- Performing survival analysis or time-to-event modeling
- Working with censored data (right-censored, left-censored, or interval-censored)
- Fitting Cox proportional hazards models (standard or penalized)
- Building ensemble survival models (Random Survival Forests, Gradient Boosting)
- Training Survival Support Vector Machines
- Evaluating survival model performance (concordance index, Brier score, time-dependent AUC)
- Estimating Kaplan-Meier or Nelson-Aalen curves
- Analyzing competing risks
- Preprocessing survival data or handling missing values in survival datasets
- Conducting any analysis using the scikit-survival library

## Core Capabilities

### 1. Model Types and Selection

scikit-survival provides multiple model families, each suited for different scenarios:

#### Cox Proportional Hazards Models
**Use for**: Standard survival analysis with interpretable coefficients
- `CoxPHSurvivalAnalysis`: Basic Cox model
- `CoxnetSurvivalAnalysis`: Penalized Cox with elastic net for high-dimensional data
- `IPCRidge`: Ridge regression for accelerated failure time models

**See**: `references/cox-models.md` for detailed guidance on Cox models, regularization, and interpretation

#### Ensemble Methods
**Use for**: High predictive performance with complex non-linear relationships
- `RandomSurvivalForest`: Robust, non-parametric ensemble method
- `GradientBoostingSurvivalAnalysis`: Tree-based boosting for maximum performance
- `ComponentwiseGradientBoostingSurvivalAnalysis`: Linear boosting with feature selection
- `ExtraSurvivalTrees`: Extremely randomized trees for additional regularization

**See**: `references/ensemble-models.md` for comprehensive guidance on ensemble methods, hyperparameter tuning, and when to use each model

#### Survival Support Vector Machines
**Use for**: Medium-sized datasets with margin-based learning
- `FastSurvivalSVM`: Linear SVM optimized for speed
- `FastKernelSurvivalSVM`: Kernel SVM for non-linear relationships
- `HingeLossSurvivalSVM`: SVM with hinge loss
- `ClinicalKernelTransform`: Specialized kernel for clinical + molecular data

**See**: `references/svm-models.md` for detailed SVM guidance, kernel selection, and hyperparameter tuning

#### Model Selection Decision Tree

```
Start
├─ High-dimensional data (p > n)?
│  ├─ Yes → CoxnetSurvivalAnalysis (elastic net)
│  └─ No → Continue
│
├─ Need interpretable coefficients?
│  ├─

... (truncated from original)
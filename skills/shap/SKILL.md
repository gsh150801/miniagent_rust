---
name: shap
description: Model interpretability and explainability using SHAP (SHapley Additive exPlanations). Use this skill when explaining machine learning model predictions, computing feature importance, generating SHAP p
triggers:
  - shap
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# SHAP (SHapley Additive exPlanations)

## Overview

SHAP is a unified approach to explain machine learning model outputs using Shapley values from cooperative game theory. This skill provides comprehensive guidance for:

- Computing SHAP values for any model type
- Creating visualizations to understand feature importance
- Debugging and validating model behavior
- Analyzing fairness and bias
- Implementing explainable AI in production

SHAP works with all model types: tree-based models (XGBoost, LightGBM, CatBoost, Random Forest), deep learning models (TensorFlow, PyTorch, Keras), linear models, and black-box models.

## When to Use This Skill

**Trigger this skill when users ask about**:
- "Explain which features are most important in my model"
- "Generate SHAP plots" (waterfall, beeswarm, bar, scatter, force, heatmap, etc.)
- "Why did my model make this prediction?"
- "Calculate SHAP values for my model"
- "Visualize feature importance using SHAP"
- "Debug my model's behavior" or "validate my model"
- "Check my model for bias" or "analyze fairness"
- "Compare feature importance across models"
- "Implement explainable AI" or "add explanations to my model"
- "Understand feature interactions"
- "Create model interpretation dashboard"

## Quick Start Guide

### Step 1: Select the Right Explainer

**Decision Tree**:

1. **Tree-based model?** (XGBoost, LightGBM, CatBoost, Random Forest, Gradient Boosting)
   - Use `shap.TreeExplainer` (fast, exact)

2. **Deep neural network?** (TensorFlow, PyTorch, Keras, CNNs, RNNs, Transformers)
   - Use `shap.DeepExplainer` or `shap.GradientExplainer`

3. **Linear model?** (Linear/Logistic Regression, GLMs)
   - Use `shap.LinearExplainer` (extremely fast)

4. **Any other model?** (SVMs, custom functions, black-box models)
   - Use `shap.KernelExplainer` (model-agnostic but slower)

5. **Unsure?**
   - Use `shap.Explainer` (automatically selects best algorithm)

**See `references/explainers.md` for detailed information on all explainer types.**

### Step 2: Compute SHAP Values

```python
import shap

# Example with tree-based model (XGBoost)
import xgboost as xgb

# Train model
model = xgb.XGBClassifier().fit(X_train, y_train)

# Create explainer
explainer = shap.TreeExplainer(model)

# Compute SHAP values
shap_values = explainer(X_test)

# The shap_values object contains:
# - values: SHAP values (feature attributions)
# - base_values: Expected model output (baseline)
# - data: Original feature values
```

### Step 3: Visualize Results

**For Global Understanding** (entire dataset):
```python
# Beeswarm plot - shows feature importance with value distributions
shap.plots.beeswarm(shap_values, max_display=15)

# Bar plot - clean summary of feature importance
shap.plots.bar(shap_values)
```

**For Individual Predictions**:
```python
# Waterfall plot - detailed breakdown of single prediction
shap.plots.waterfall(shap_values[0])

# Force plot - additive force visualization
shap.plots.force(shap_values[0])
```

**For Fe

... (truncated from original)
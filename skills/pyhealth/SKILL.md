---
name: pyhealth
description: Build clinical/healthcare deep-learning pipelines with PyHealth — loading EHR/signal/imaging datasets (MIMIC-III/IV, eICU, OMOP, SleepEDF, ChestXray14, EHRShot), defining tasks (mortality, readmission
triggers:
  - pyhealth
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# PyHealth

PyHealth (https://pyhealth.dev/) is a Python toolkit for clinical deep learning. It provides a unified, modular pipeline across electronic health records (EHR), physiological signals, and medical imaging.

The library is built around a **5-stage pipeline** — `Dataset → Task → Model → Trainer → Metrics` — where each stage is replaceable and the interfaces between stages are stable. Code that follows this pipeline shape composes well; code that bypasses it usually fights the library.

## When to use this skill

Use this skill whenever the user is doing clinical/healthcare ML and any of the following are true:

- They mention PyHealth, MIMIC-III/IV, eICU, OMOP-CDM, EHRShot, SleepEDF, SHHS, ISRUC, COVID19-CXR, ChestX-ray14, TUEV/TUAB.
- They want to predict mortality, readmission, length of stay, drug recommendations, sleep stages, ICD codes, EEG events, or de-identification.
- They need to look up or cross-map medical codes (ICD-9-CM, ICD-10-CM, ATC, NDC, RxNorm, CCS).
- They have EHR-shaped data and want to train a clinical model without writing the plumbing themselves.

PyHealth is the right tool when the workflow fits its 5 stages. If the user just wants generic PyTorch on tabular data, this skill is not necessary.

## Installation (uv)

PyHealth 2.0 requires Python ≥ 3.12, < 3.14. Use `uv` for environment management — it's faster and reproducible.

```bash
# Create a project with the right Python
uv init my-pyhealth-project
cd my-pyhealth-project
uv python pin 3.12

# Add PyHealth (this also pulls in PyTorch and friends)
uv add pyhealth

# Run scripts inside the env
uv run python train.py
```

For a one-off script without a project, use `uv run --with pyhealth python script.py`. For the legacy 1.x line (Python 3.9+), `uv add pyhealth==1.16`. Detailed install notes, MIMIC access, and GPU/CPU device tips are in `references/installation.md`.

## The 5-stage pipeline

A complete pipeline is typically <20 lines. This is the canonical shape — start here and modify pieces:

```python
from pyhealth.datasets import MIMIC3Dataset, split_by_patient, get_dataloader
from pyhealth.tasks import MortalityPredictionMIMIC3
from pyhealth.models import Transformer
from pyhealth.trainer import Trainer
from pyhealth.metrics.binary import binary_metrics_fn

# 1. Dataset — raw patient registry
base = MIMIC3Dataset(
    root="https://storage.googleapis.com/pyhealth/Synthetic_MIMIC-III/",
    tables=["DIAGNOSES_ICD", "PROCEDURES_ICD", "PRESCRIPTIONS"],
)

# 2. Task — converts patients into supervised samples
samples = base.set_task(MortalityPredictionMIMIC3())

# 3. Split + DataLoaders (split by patient to avoid leakage)
train_ds, val_ds, test_ds = split_by_patient(samples, [0.8, 0.1, 0.1])
train_loader = get_dataloader(train_ds, batch_size=32, shuffle=True)
val_loader   = get_dataloader(val_ds,   batch_size=32, shuffle=False)
test_loader  = get_dataloader(test_ds,  batch_size=32, shuffle=False)

# 4. Model — must be passed the SampleDataset, not the BaseData

... (truncated from original)
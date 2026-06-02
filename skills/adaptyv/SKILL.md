---
name: adaptyv
description: "How to use the Adaptyv Bio Foundry API and Python SDK for protein experiment design, submission, and results retrieval. Use this skill whenever the user mentions Adaptyv, Foundry API, protein binding
triggers:
  - adaptyv
tools_needed:
version: "0.1.0"
priority: 5
---

# Adaptyv Bio Foundry API

Adaptyv Bio is a cloud lab that turns protein sequences into experimental data. Users submit amino acid sequences via API or UI; Adaptyv's automated lab runs assays (binding, thermostability, expression, fluorescence) and delivers results in ~21 days.

## Quick Start

**Base URL:** `https://foundry-api-public.adaptyvbio.com/api/v1`

**Authentication:** Bearer token in the `Authorization` header. Tokens are obtained from [foundry.adaptyvbio.com](https://foundry.adaptyvbio.com/) sidebar.

When writing code, always read the API key from the environment variable `ADAPTYV_API_KEY` or from a `.env` file — never hardcode tokens. Check for a `.env` file in the project root first; if one exists, use a library like `python-dotenv` to load it.

```bash
export FOUNDRY_API_TOKEN="abs0_..."
curl https://foundry-api-public.adaptyvbio.com/api/v1/targets?limit=3 \
  -H "Authorization: Bearer $FOUNDRY_API_TOKEN"
```

Every request except `GET /openapi.json` requires authentication. Store tokens in environment variables or `.env` files — never commit them to source control.

## Python SDK

Install: `uv add adaptyv-sdk` (falls back to `uv pip install adaptyv-sdk` if no `pyproject.toml` exists)

**Environment variables** (set in shell or `.env` file):
```bash
ADAPTYV_API_KEY=your_api_key
ADAPTYV_API_URL=https://foundry-api-public.adaptyvbio.com/api/v1
```

### Decorator Pattern

```python
from adaptyv import lab

@lab.experiment(target="PD-L1", experiment_type="screening", method="bli")
def design_binders():
    return {"design_a": "MVKVGVNG...", "design_b": "MKVLVAG..."}

result = design_binders()
print(f"Experiment: {result.experiment_url}")
```

### Client Pattern

```python
from adaptyv import FoundryClient

client = FoundryClient(api_key="...", base_url="https://foundry-api-public.adaptyvbio.com/api/v1")

# Browse targets
targets = client.targets.list(search="EGFR", selfservice_only=True)

# Estimate cost
estimate = client.experiments.cost_estimate({
    "experiment_spec": {
        "experiment_type": "screening",
        "method": "bli",
        "target_id": "target-uuid",
        "sequences": {"seq1": "EVQLVESGGGLVQ..."},
        "n_replicates": 3
    }
})

# Create and submit
exp = client.experiments.create({...})
client.experiments.submit(exp.experiment_id)

# Later: retrieve results
results = client.experiments.get_results(exp.experiment_id)
```

## Experiment Types

| Type | Method | Measures | Requires Target |
|---|---|---|---|
| `affinity` | `bli` or `spr` | KD, kon, koff kinetics | Yes |
| `screening` | `bli` or `spr` | Yes/no binding | Yes |
| `thermostability` | — | Melting temperature (Tm) | No |
| `expression` | — | Expression yield | No |
| `fluorescence` | — | Fluorescence intensity | No |

## Experiment Lifecycle

```
Draft → WaitingForConfirmation → QuoteSent → WaitingForMaterials → InQueue → InProduction → DataAnalysis → InReview → Done
```

| Status | Who Acts | Description |
|---|---|---|
| `Draft` | You | Editable

... (truncated from original)
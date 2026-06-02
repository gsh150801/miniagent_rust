---
name: modal
description: Cloud computing platform for running Python on GPUs and serverless infrastructure. Use when deploying AI/ML models, running GPU-accelerated workloads, serving web endpoints, scheduling batch jobs, or 
triggers:
  - modal
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Modal

## Overview

Modal is a cloud platform for running Python code serverlessly, with a focus on AI/ML workloads. Key capabilities:
- **GPU compute** on demand (T4, L4, A10, L40S, A100, H100, H200, B200)
- **Serverless functions** with autoscaling from zero to thousands of containers
- **Custom container images** built entirely in Python code
- **Persistent storage** via Volumes for model weights and datasets
- **Web endpoints** for serving models and APIs
- **Scheduled jobs** via cron or fixed intervals
- **Sub-second cold starts** for low-latency inference

Everything in Modal is defined as code — no YAML, no Dockerfiles required (though both are supported).

## When to Use This Skill

Use this skill when:
- Deploy or serve AI/ML models in the cloud
- Run GPU-accelerated computations (training, inference, fine-tuning)
- Create serverless web APIs or endpoints
- Scale batch processing jobs in parallel
- Schedule recurring tasks (data pipelines, retraining, scraping)
- Need persistent cloud storage for model weights or datasets
- Want to run code in custom container environments
- Build job queues or async task processing systems

## Installation and Authentication

### Install

```bash
uv pip install modal
```

### Authenticate

Prefer existing credentials before creating new ones:

1. Check whether `MODAL_TOKEN_ID` and `MODAL_TOKEN_SECRET` are already present in the current environment.
2. If not, check for those values in a local `.env` file and load them if appropriate for the workflow.
3. Only fall back to interactive `modal setup` or generating fresh tokens if neither source already provides credentials.

```bash
modal setup
```

This opens a browser for authentication. For CI/CD or headless environments, use environment variables:

```bash
export MODAL_TOKEN_ID=<your-token-id>
export MODAL_TOKEN_SECRET=<your-token-secret>
```

If tokens are not already available in the environment or `.env`, generate them at https://modal.com/settings

Modal offers a free tier with $30/month in credits.

**Reference**: See `references/getting-started.md` for detailed setup and first app walkthrough.

## Core Concepts

### App and Functions

A Modal `App` groups related functions. Functions decorated with `@app.function()` run remotely in the cloud:

```python
import modal

app = modal.App("my-app")

@app.function()
def square(x):
    return x ** 2

@app.local_entrypoint()
def main():
    # .remote() runs in the cloud
    print(square.remote(42))
```

Run with `modal run script.py`. Deploy with `modal deploy script.py`.

**Reference**: See `references/functions.md` for lifecycle hooks, classes, `.map()`, `.spawn()`, and more.

### Container Images

Modal builds container images from Python code. The recommended package installer is `uv`:

```python
image = (
    modal.Image.debian_slim(python_version="3.11")
    .uv_pip_install("torch==2.8.0", "transformers", "accelerate")
    .apt_install("git")
)

@app.function(image=image)
def inference(prompt):
   

... (truncated from original)
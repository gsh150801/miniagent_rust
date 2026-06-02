---
name: pufferlib
description: High-performance reinforcement learning framework optimized for speed and scale. Use when you need fast parallel training, vectorized environments, multi-agent systems, or integration with game enviro
triggers:
  - pufferlib
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# PufferLib - High-Performance Reinforcement Learning

## Overview

PufferLib is a high-performance reinforcement learning library designed for fast parallel environment simulation and training. It achieves training at millions of steps per second through optimized vectorization, native multi-agent support, and efficient PPO implementation (PuffeRL). The library provides the Ocean suite of 20+ environments and seamless integration with Gymnasium, PettingZoo, and specialized RL frameworks.

## When to Use This Skill

Use this skill when:
- **Training RL agents** with PPO on any environment (single or multi-agent)
- **Creating custom environments** using the PufferEnv API
- **Optimizing performance** for parallel environment simulation (vectorization)
- **Integrating existing environments** from Gymnasium, PettingZoo, Atari, Procgen, etc.
- **Developing policies** with CNN, LSTM, or custom architectures
- **Scaling RL** to millions of steps per second for faster experimentation
- **Multi-agent RL** with native multi-agent environment support

## Core Capabilities

### 1. High-Performance Training (PuffeRL)

PuffeRL is PufferLib's optimized PPO+LSTM training algorithm achieving 1M-4M steps/second.

**Quick start training:**
```bash
# CLI training
puffer train procgen-coinrun --train.device cuda --train.learning-rate 3e-4

# Distributed training
torchrun --nproc_per_node=4 train.py
```

**Python training loop:**
```python
import pufferlib
from pufferlib import PuffeRL

# Create vectorized environment
env = pufferlib.make('procgen-coinrun', num_envs=256)

# Create trainer
trainer = PuffeRL(
    env=env,
    policy=my_policy,
    device='cuda',
    learning_rate=3e-4,
    batch_size=32768
)

# Training loop
for iteration in range(num_iterations):
    trainer.evaluate()  # Collect rollouts
    trainer.train()     # Train on batch
    trainer.mean_and_log()  # Log results
```

**For comprehensive training guidance**, read `references/training.md` for:
- Complete training workflow and CLI options
- Hyperparameter tuning with Protein
- Distributed multi-GPU/multi-node training
- Logger integration (Weights & Biases, Neptune)
- Checkpointing and resume training
- Performance optimization tips
- Curriculum learning patterns

### 2. Environment Development (PufferEnv)

Create custom high-performance environments with the PufferEnv API.

**Basic environment structure:**
```python
import numpy as np
from pufferlib import PufferEnv

class MyEnvironment(PufferEnv):
    def __init__(self, buf=None):
        super().__init__(buf)

        # Define spaces
        self.observation_space = self.make_space((4,))
        self.action_space = self.make_discrete(4)

        self.reset()

    def reset(self):
        # Reset state and return initial observation
        return np.zeros(4, dtype=np.float32)

    def step(self, action):
        # Execute action, compute reward, check done
        obs = self._get_observation()
        reward = self._compute_reward()
        don

... (truncated from original)
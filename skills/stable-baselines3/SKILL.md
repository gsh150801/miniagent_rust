---
name: stable-baselines3
description: Production-ready reinforcement learning algorithms (PPO, SAC, DQN, TD3, DDPG, A2C) with scikit-learn-like API. Use for standard RL experiments, quick prototyping, and well-documented algorithm impleme
triggers:
  - stable baselines3
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Stable Baselines3

## Overview

Stable Baselines3 (SB3) is a PyTorch-based library providing reliable implementations of reinforcement learning algorithms. This skill provides comprehensive guidance for training RL agents, creating custom environments, implementing callbacks, and optimizing training workflows using SB3's unified API.

## Core Capabilities

### 1. Training RL Agents

**Basic Training Pattern:**

```python
import gymnasium as gym
from stable_baselines3 import PPO

# Create environment
env = gym.make("CartPole-v1")

# Initialize agent
model = PPO("MlpPolicy", env, verbose=1)

# Train the agent
model.learn(total_timesteps=10000)

# Save the model
model.save("ppo_cartpole")

# Load the model (without prior instantiation)
model = PPO.load("ppo_cartpole", env=env)
```

**Important Notes:**
- `total_timesteps` is a lower bound; actual training may exceed this due to batch collection
- Use `model.load()` as a static method, not on an existing instance
- The replay buffer is NOT saved with the model to save space

**Algorithm Selection:**
Use `references/algorithms.md` for detailed algorithm characteristics and selection guidance. Quick reference:
- **PPO/A2C**: General-purpose, supports all action space types, good for multiprocessing
- **SAC/TD3**: Continuous control, off-policy, sample-efficient
- **DQN**: Discrete actions, off-policy
- **HER**: Goal-conditioned tasks

See `scripts/train_rl_agent.py` for a complete training template with best practices.

### 2. Custom Environments

**Requirements:**
Custom environments must inherit from `gymnasium.Env` and implement:
- `__init__()`: Define action_space and observation_space
- `reset(seed, options)`: Return initial observation and info dict
- `step(action)`: Return observation, reward, terminated, truncated, info
- `render()`: Visualization (optional)
- `close()`: Cleanup resources

**Key Constraints:**
- Image observations must be `np.uint8` in range [0, 255]
- Use channel-first format when possible (channels, height, width)
- SB3 normalizes images automatically by dividing by 255
- Set `normalize_images=False` in policy_kwargs if pre-normalized
- SB3 does NOT support `Discrete` or `MultiDiscrete` spaces with `start!=0`

**Validation:**
```python
from stable_baselines3.common.env_checker import check_env

check_env(env, warn=True)
```

See `scripts/custom_env_template.py` for a complete custom environment template and `references/custom_environments.md` for comprehensive guidance.

### 3. Vectorized Environments

**Purpose:**
Vectorized environments run multiple environment instances in parallel, accelerating training and enabling certain wrappers (frame-stacking, normalization).

**Types:**
- **DummyVecEnv**: Sequential execution on current process (for lightweight environments)
- **SubprocVecEnv**: Parallel execution across processes (for compute-heavy environments)

**Quick Setup:**
```python
from stable_baselines3.common.env_util import make_vec_env

# Create 4 parallel environmen

... (truncated from original)
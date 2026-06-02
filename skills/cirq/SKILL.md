---
name: cirq
description: Google quantum computing framework. Use when targeting Google Quantum AI hardware, designing noise-aware circuits, or running quantum characterization experiments. Best for Google hardware, noise mode
triggers:
  - cirq
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Cirq - Quantum Computing with Python

Cirq is Google Quantum AI's open-source framework for designing, simulating, and running quantum circuits on quantum computers and simulators.

## Installation

```bash
uv pip install cirq
```

For hardware integration:
```bash
# Google Quantum Engine
uv pip install cirq-google

# IonQ
uv pip install cirq-ionq

# AQT (Alpine Quantum Technologies)
uv pip install cirq-aqt

# Pasqal
uv pip install cirq-pasqal

# Azure Quantum
uv pip install azure-quantum cirq
```

## Quick Start

### Basic Circuit

```python
import cirq
import numpy as np

# Create qubits
q0, q1 = cirq.LineQubit.range(2)

# Build circuit
circuit = cirq.Circuit(
    cirq.H(q0),              # Hadamard on q0
    cirq.CNOT(q0, q1),       # CNOT with q0 control, q1 target
    cirq.measure(q0, q1, key='result')
)

print(circuit)

# Simulate
simulator = cirq.Simulator()
result = simulator.run(circuit, repetitions=1000)

# Display results
print(result.histogram(key='result'))
```

### Parameterized Circuit

```python
import sympy

# Define symbolic parameter
theta = sympy.Symbol('theta')

# Create parameterized circuit
circuit = cirq.Circuit(
    cirq.ry(theta)(q0),
    cirq.measure(q0, key='m')
)

# Sweep over parameter values
sweep = cirq.Linspace('theta', start=0, stop=2*np.pi, length=20)
results = simulator.run_sweep(circuit, params=sweep, repetitions=1000)

# Process results
for params, result in zip(sweep, results):
    theta_val = params['theta']
    counts = result.histogram(key='m')
    print(f"θ={theta_val:.2f}: {counts}")
```

## Core Capabilities

### Circuit Building
For comprehensive information about building quantum circuits, including qubits, gates, operations, custom gates, and circuit patterns, see:
- **[references/building.md](references/building.md)** - Complete guide to circuit construction

Common topics:
- Qubit types (GridQubit, LineQubit, NamedQubit)
- Single and two-qubit gates
- Parameterized gates and operations
- Custom gate decomposition
- Circuit organization with moments
- Standard circuit patterns (Bell states, GHZ, QFT)
- Import/export (OpenQASM, JSON)
- Working with qudits and observables

### Simulation
For detailed information about simulating quantum circuits, including exact simulation, noisy simulation, parameter sweeps, and the Quantum Virtual Machine, see:
- **[references/simulation.md](references/simulation.md)** - Complete guide to quantum simulation

Common topics:
- Exact simulation (state vector, density matrix)
- Sampling and measurements
- Parameter sweeps (single and multiple parameters)
- Noisy simulation
- State histograms and visualization
- Quantum Virtual Machine (QVM)
- Expectation values and observables
- Performance optimization

### Circuit Transformation
For information about optimizing, compiling, and manipulating quantum circuits, see:
- **[references/transformation.md](references/transformation.md)** - Complete guide to circuit transformations

Common topics:
- Transformer framework
- Ga

... (truncated from original)
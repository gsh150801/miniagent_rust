---
name: fluidsim
description: Framework for computational fluid dynamics simulations using Python. Use when running fluid dynamics simulations including Navier-Stokes equations (2D/3D), shallow water equations, stratified flows, o
triggers:
  - fluidsim
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# FluidSim

## Overview

FluidSim is an object-oriented Python framework for high-performance computational fluid dynamics (CFD) simulations. It provides solvers for periodic-domain equations using pseudospectral methods with FFT, delivering performance comparable to Fortran/C++ while maintaining Python's ease of use.

**Key strengths**:
- Multiple solvers: 2D/3D Navier-Stokes, shallow water, stratified flows
- High performance: Pythran/Transonic compilation, MPI parallelization
- Complete workflow: Parameter configuration, simulation execution, output analysis
- Interactive analysis: Python-based post-processing and visualization

## Core Capabilities

### 1. Installation and Setup

Install fluidsim using uv with appropriate feature flags:

```bash
# Basic installation
uv pip install fluidsim

# With FFT support (required for most solvers)
uv pip install "fluidsim[fft]"

# With MPI for parallel computing
uv pip install "fluidsim[fft,mpi]"
```

Set environment variables for output directories (optional):

```bash
export FLUIDSIM_PATH=/path/to/simulation/outputs
export FLUIDDYN_PATH_SCRATCH=/path/to/working/directory
```

No API keys or authentication required.

See `references/installation.md` for complete installation instructions and environment configuration.

### 2. Running Simulations

Standard workflow consists of five steps:

**Step 1**: Import solver
```python
from fluidsim.solvers.ns2d.solver import Simul
```

**Step 2**: Create and configure parameters
```python
params = Simul.create_default_params()
params.oper.nx = params.oper.ny = 256
params.oper.Lx = params.oper.Ly = 2 * 3.14159
params.nu_2 = 1e-3
params.time_stepping.t_end = 10.0
params.init_fields.type = "noise"
```

**Step 3**: Instantiate simulation
```python
sim = Simul(params)
```

**Step 4**: Execute
```python
sim.time_stepping.start()
```

**Step 5**: Analyze results
```python
sim.output.phys_fields.plot("vorticity")
sim.output.spatial_means.plot()
```

See `references/simulation_workflow.md` for complete examples, restarting simulations, and cluster deployment.

### 3. Available Solvers

Choose solver based on physical problem:

**2D Navier-Stokes** (`ns2d`): 2D turbulence, vortex dynamics
```python
from fluidsim.solvers.ns2d.solver import Simul
```

**3D Navier-Stokes** (`ns3d`): 3D turbulence, realistic flows
```python
from fluidsim.solvers.ns3d.solver import Simul
```

**Stratified flows** (`ns2d.strat`, `ns3d.strat`): Oceanic/atmospheric flows
```python
from fluidsim.solvers.ns2d.strat.solver import Simul
params.N = 1.0  # Brunt-Väisälä frequency
```

**Shallow water** (`sw1l`): Geophysical flows, rotating systems
```python
from fluidsim.solvers.sw1l.solver import Simul
params.f = 1.0  # Coriolis parameter
```

See `references/solvers.md` for complete solver list and selection guidance.

### 4. Parameter Configuration

Parameters are organized hierarchically and accessed via dot notation:

**Domain and resolution**:
```python
params.oper.nx = 256  # grid points
params.o

... (truncated from original)
---
name: neurokit2
description: Comprehensive biosignal processing toolkit for analyzing physiological data including ECG, EEG, EDA, RSP, PPG, EMG, and EOG signals. Use this skill when processing cardiovascular signals, brain activi
triggers:
  - neurokit2
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# NeuroKit2

## Overview

NeuroKit2 is a comprehensive Python toolkit for processing and analyzing physiological signals (biosignals). Use this skill to process cardiovascular, neural, autonomic, respiratory, and muscular signals for psychophysiology research, clinical applications, and human-computer interaction studies.

## When to Use This Skill

Apply this skill when working with:
- **Cardiac signals**: ECG, PPG, heart rate variability (HRV), pulse analysis
- **Brain signals**: EEG frequency bands, microstates, complexity, source localization
- **Autonomic signals**: Electrodermal activity (EDA/GSR), skin conductance responses (SCR)
- **Respiratory signals**: Breathing rate, respiratory variability (RRV), volume per time
- **Muscular signals**: EMG amplitude, muscle activation detection
- **Eye tracking**: EOG, blink detection and analysis
- **Multi-modal integration**: Processing multiple physiological signals simultaneously
- **Complexity analysis**: Entropy measures, fractal dimensions, nonlinear dynamics

## Core Capabilities

### 1. Cardiac Signal Processing (ECG/PPG)

Process electrocardiogram and photoplethysmography signals for cardiovascular analysis. See `references/ecg_cardiac.md` for detailed workflows.

**Primary workflows:**
- ECG processing pipeline: cleaning → R-peak detection → delineation → quality assessment
- HRV analysis across time, frequency, and nonlinear domains
- PPG pulse analysis and quality assessment
- ECG-derived respiration extraction

**Key functions:**
```python
import neurokit2 as nk

# Complete ECG processing pipeline
signals, info = nk.ecg_process(ecg_signal, sampling_rate=1000)

# Analyze ECG data (event-related or interval-related)
analysis = nk.ecg_analyze(signals, sampling_rate=1000)

# Comprehensive HRV analysis
hrv = nk.hrv(peaks, sampling_rate=1000)  # Time, frequency, nonlinear domains
```

### 2. Heart Rate Variability Analysis

Compute comprehensive HRV metrics from cardiac signals. See `references/hrv.md` for all indices and domain-specific analysis.

**Supported domains:**
- **Time domain**: SDNN, RMSSD, pNN50, SDSD, and derived metrics
- **Frequency domain**: ULF, VLF, LF, HF, VHF power and ratios
- **Nonlinear domain**: Poincaré plot (SD1/SD2), entropy measures, fractal dimensions
- **Specialized**: Respiratory sinus arrhythmia (RSA), recurrence quantification analysis (RQA)

**Key functions:**
```python
# All HRV indices at once
hrv_indices = nk.hrv(peaks, sampling_rate=1000)

# Domain-specific analysis
hrv_time = nk.hrv_time(peaks)
hrv_freq = nk.hrv_frequency(peaks, sampling_rate=1000)
hrv_nonlinear = nk.hrv_nonlinear(peaks, sampling_rate=1000)
hrv_rsa = nk.hrv_rsa(peaks, rsp_signal, sampling_rate=1000)
```

### 3. Brain Signal Analysis (EEG)

Analyze electroencephalography signals for frequency power, complexity, and microstate patterns. See `references/eeg.md` for detailed workflows and MNE integration.

**Primary capabilities:**
- Frequency band power analysis (Delta, Theta, Alpha, Beta,

... (truncated from original)
---
name: aeon
description: This skill should be used for time series machine learning tasks including classification, regression, clustering, forecasting, anomaly detection, segmentation, and similarity search. Use when working
triggers:
  - aeon
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Aeon Time Series Machine Learning

## Overview

Aeon is a scikit-learn compatible Python toolkit for time series machine learning. It provides state-of-the-art algorithms for classification, regression, clustering, forecasting, anomaly detection, segmentation, and similarity search.

## When to Use This Skill

Apply this skill when:
- Classifying or predicting from time series data
- Detecting anomalies or change points in temporal sequences
- Clustering similar time series patterns
- Forecasting future values
- Finding repeated patterns (motifs) or unusual subsequences (discords)
- Comparing time series with specialized distance metrics
- Extracting features from temporal data

## Installation

```bash
uv pip install aeon
```

## Core Capabilities

### 1. Time Series Classification

Categorize time series into predefined classes. See `references/classification.md` for complete algorithm catalog.

**Quick Start:**
```python
from aeon.classification.convolution_based import RocketClassifier
from aeon.datasets import load_classification

# Load data
X_train, y_train = load_classification("GunPoint", split="train")
X_test, y_test = load_classification("GunPoint", split="test")

# Train classifier
clf = RocketClassifier(n_kernels=10000)
clf.fit(X_train, y_train)
accuracy = clf.score(X_test, y_test)
```

**Algorithm Selection:**
- **Speed + Performance**: `MiniRocketClassifier`, `Arsenal`
- **Maximum Accuracy**: `HIVECOTEV2`, `InceptionTimeClassifier`
- **Interpretability**: `ShapeletTransformClassifier`, `Catch22Classifier`
- **Small Datasets**: `KNeighborsTimeSeriesClassifier` with DTW distance

### 2. Time Series Regression

Predict continuous values from time series. See `references/regression.md` for algorithms.

**Quick Start:**
```python
from aeon.regression.convolution_based import RocketRegressor
from aeon.datasets import load_regression

X_train, y_train = load_regression("Covid3Month", split="train")
X_test, y_test = load_regression("Covid3Month", split="test")

reg = RocketRegressor()
reg.fit(X_train, y_train)
predictions = reg.predict(X_test)
```

### 3. Time Series Clustering

Group similar time series without labels. See `references/clustering.md` for methods.

**Quick Start:**
```python
from aeon.clustering import TimeSeriesKMeans

clusterer = TimeSeriesKMeans(
    n_clusters=3,
    distance="dtw",
    averaging_method="ba"
)
labels = clusterer.fit_predict(X_train)
centers = clusterer.cluster_centers_
```

### 4. Forecasting

Predict future time series values. See `references/forecasting.md` for forecasters.

**Quick Start:**
```python
from aeon.forecasting.arima import ARIMA

forecaster = ARIMA(order=(1, 1, 1))
forecaster.fit(y_train)
y_pred = forecaster.predict(fh=[1, 2, 3, 4, 5])
```

### 5. Anomaly Detection

Identify unusual patterns or outliers. See `references/anomaly_detection.md` for detectors.

**Quick Start:**
```python
from aeon.anomaly_detection import STOMP

detector = STOMP(window_size=50)
anomaly_scores = detector.fit

... (truncated from original)
---
name: geomaster
description: Comprehensive geospatial science skill covering remote sensing, GIS, spatial analysis, machine learning for earth observation, and 30+ scientific domains. Supports satellite imagery processing (Sentin
triggers:
  - geomaster
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# GeoMaster

Comprehensive geospatial science skill covering GIS, remote sensing, spatial analysis, and ML for Earth observation across 70+ topics with 500+ code examples in 8 programming languages.

## Installation

```bash
# Core Python stack (conda recommended)
conda install -c conda-forge gdal rasterio fiona shapely pyproj geopandas

# Remote sensing & ML
uv pip install rsgislib torchgeo earthengine-api
uv pip install scikit-learn xgboost torch-geometric

# Network & visualization
uv pip install osmnx networkx folium keplergl
uv pip install cartopy contextily mapclassify

# Big data & cloud
uv pip install xarray rioxarray dask-geopandas
uv pip install pystac-client planetary-computer

# Point clouds
uv pip install laspy pylas open3d pdal

# Databases
conda install -c conda-forge postgis spatialite
```

## Quick Start

### NDVI from Sentinel-2

```python
import rasterio
import numpy as np

with rasterio.open('sentinel2.tif') as src:
    red = src.read(4).astype(float)   # B04
    nir = src.read(8).astype(float)   # B08
    ndvi = (nir - red) / (nir + red + 1e-8)
    ndvi = np.nan_to_num(ndvi, nan=0)

    profile = src.profile
    profile.update(count=1, dtype=rasterio.float32)

    with rasterio.open('ndvi.tif', 'w', **profile) as dst:
        dst.write(ndvi.astype(rasterio.float32), 1)
```

### Spatial Analysis with GeoPandas

```python
import geopandas as gpd

# Load and ensure same CRS
zones = gpd.read_file('zones.geojson')
points = gpd.read_file('points.geojson')

if zones.crs != points.crs:
    points = points.to_crs(zones.crs)

# Spatial join and statistics
joined = gpd.sjoin(points, zones, how='inner', predicate='within')
stats = joined.groupby('zone_id').agg({
    'value': ['count', 'mean', 'std', 'min', 'max']
}).round(2)
```

### Google Earth Engine Time Series

```python
import ee
import pandas as pd

ee.Initialize(project='your-project')
roi = ee.Geometry.Point([-122.4, 37.7]).buffer(10000)

s2 = (ee.ImageCollection('COPERNICUS/S2_SR_HARMONIZED')
      .filterBounds(roi)
      .filterDate('2020-01-01', '2023-12-31')
      .filter(ee.Filter.lt('CLOUDY_PIXEL_PERCENTAGE', 20)))

def add_ndvi(img):
    return img.addBands(img.normalizedDifference(['B8', 'B4']).rename('NDVI'))

s2_ndvi = s2.map(add_ndvi)

def extract_series(image):
    stats = image.reduceRegion(ee.Reducer.mean(), roi.centroid(), scale=10, maxPixels=1e9)
    return ee.Feature(None, {'date': image.date().format('YYYY-MM-dd'), 'ndvi': stats.get('NDVI')})

series = s2_ndvi.map(extract_series).getInfo()
df = pd.DataFrame([f['properties'] for f in series['features']])
df['date'] = pd.to_datetime(df['date'])
```

## Core Concepts

### Data Types

| Type | Examples | Libraries |
|------|----------|-----------|
| Vector | Shapefile, GeoJSON, GeoPackage | GeoPandas, Fiona, GDAL |
| Raster | GeoTIFF, NetCDF, COG | Rasterio, Xarray, GDAL |
| Point Cloud | LAS, LAZ | Laspy, PDAL

... (truncated from original)
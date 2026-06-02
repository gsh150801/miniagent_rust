---
name: geopandas
description: Python library for working with geospatial vector data including shapefiles, GeoJSON, and GeoPackage files. Use when working with geographic data for spatial analysis, geometric operations, coordinate
triggers:
  - geopandas
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# GeoPandas

GeoPandas extends pandas to enable spatial operations on geometric types. It combines the capabilities of pandas and shapely for geospatial data analysis.

## Installation

```bash
uv pip install geopandas
```

### Optional Dependencies

```bash
# For interactive maps
uv pip install folium

# For classification schemes in mapping
uv pip install mapclassify

# For faster I/O operations (2-4x speedup)
uv pip install pyarrow

# For PostGIS database support
uv pip install psycopg2
uv pip install geoalchemy2

# For basemaps
uv pip install contextily

# For cartographic projections
uv pip install cartopy
```

## Quick Start

```python
import geopandas as gpd

# Read spatial data
gdf = gpd.read_file("data.geojson")

# Basic exploration
print(gdf.head())
print(gdf.crs)
print(gdf.geometry.geom_type)

# Simple plot
gdf.plot()

# Reproject to different CRS
gdf_projected = gdf.to_crs("EPSG:3857")

# Calculate area (use projected CRS for accuracy)
gdf_projected['area'] = gdf_projected.geometry.area

# Save to file
gdf.to_file("output.gpkg")
```

## Core Concepts

### Data Structures

- **GeoSeries**: Vector of geometries with spatial operations
- **GeoDataFrame**: Tabular data structure with geometry column

See [data-structures.md](references/data-structures.md) for details.

### Reading and Writing Data

GeoPandas reads/writes multiple formats: Shapefile, GeoJSON, GeoPackage, PostGIS, Parquet.

```python
# Read with filtering
gdf = gpd.read_file("data.gpkg", bbox=(xmin, ymin, xmax, ymax))

# Write with Arrow acceleration
gdf.to_file("output.gpkg", use_arrow=True)
```

See [data-io.md](references/data-io.md) for comprehensive I/O operations.

### Coordinate Reference Systems

Always check and manage CRS for accurate spatial operations:

```python
# Check CRS
print(gdf.crs)

# Reproject (transforms coordinates)
gdf_projected = gdf.to_crs("EPSG:3857")

# Set CRS (only when metadata missing)
gdf = gdf.set_crs("EPSG:4326")
```

See [crs-management.md](references/crs-management.md) for CRS operations.

## Common Operations

### Geometric Operations

Buffer, simplify, centroid, convex hull, affine transformations:

```python
# Buffer by 10 units
buffered = gdf.geometry.buffer(10)

# Simplify with tolerance
simplified = gdf.geometry.simplify(tolerance=5, preserve_topology=True)

# Get centroids
centroids = gdf.geometry.centroid
```

See [geometric-operations.md](references/geometric-operations.md) for all operations.

### Spatial Analysis

Spatial joins, overlay operations, dissolve:

```python
# Spatial join (intersects)
joined = gpd.sjoin(gdf1, gdf2, predicate='intersects')

# Nearest neighbor join
nearest = gpd.sjoin_nearest(gdf1, gdf2, max_distance=1000)

# Overlay intersection
intersection = gpd.overlay(gdf1, gdf2, how='intersection')

# Dissolve by attribute
dissolved = gdf.dissolve(by='region', aggfunc='sum')
```

See [spatial-analysis.md](references/spatial-analysis.md) for analysis operations.

### Visualization

Create static and interactive 

... (truncated from original)
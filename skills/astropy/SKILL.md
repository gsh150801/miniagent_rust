---
name: astropy
description: Comprehensive Python library for astronomy and astrophysics. This skill should be used when working with astronomical data including celestial coordinates, physical units, FITS files, cosmological cal
triggers:
  - astropy
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 5
---

# Astropy

## Overview

Astropy is the core Python package for astronomy, providing essential functionality for astronomical research and data analysis. Use astropy for coordinate transformations, unit and quantity calculations, FITS file operations, cosmological calculations, precise time handling, tabular data manipulation, and astronomical image processing.

## When to Use This Skill

Use astropy when tasks involve:
- Converting between celestial coordinate systems (ICRS, Galactic, FK5, AltAz, etc.)
- Working with physical units and quantities (converting Jy to mJy, parsecs to km, etc.)
- Reading, writing, or manipulating FITS files (images or tables)
- Cosmological calculations (luminosity distance, lookback time, Hubble parameter)
- Precise time handling with different time scales (UTC, TAI, TT, TDB) and formats (JD, MJD, ISO)
- Table operations (reading catalogs, cross-matching, filtering, joining)
- WCS transformations between pixel and world coordinates
- Astronomical constants and calculations

## Quick Start

```python
import astropy.units as u
from astropy.coordinates import SkyCoord
from astropy.time import Time
from astropy.io import fits
from astropy.table import Table
from astropy.cosmology import Planck18

# Units and quantities
distance = 100 * u.pc
distance_km = distance.to(u.km)

# Coordinates
coord = SkyCoord(ra=10.5*u.degree, dec=41.2*u.degree, frame='icrs')
coord_galactic = coord.galactic

# Time
t = Time('2023-01-15 12:30:00')
jd = t.jd  # Julian Date

# FITS files
data = fits.getdata('image.fits')
header = fits.getheader('image.fits')

# Tables
table = Table.read('catalog.fits')

# Cosmology
d_L = Planck18.luminosity_distance(z=1.0)
```

## Core Capabilities

### 1. Units and Quantities (`astropy.units`)

Handle physical quantities with units, perform unit conversions, and ensure dimensional consistency in calculations.

**Key operations:**
- Create quantities by multiplying values with units
- Convert between units using `.to()` method
- Perform arithmetic with automatic unit handling
- Use equivalencies for domain-specific conversions (spectral, doppler, parallax)
- Work with logarithmic units (magnitudes, decibels)

**See:** `references/units.md` for comprehensive documentation, unit systems, equivalencies, performance optimization, and unit arithmetic.

### 2. Coordinate Systems (`astropy.coordinates`)

Represent celestial positions and transform between different coordinate frames.

**Key operations:**
- Create coordinates with `SkyCoord` in any frame (ICRS, Galactic, FK5, AltAz, etc.)
- Transform between coordinate systems
- Calculate angular separations and position angles
- Match coordinates to catalogs
- Include distance for 3D coordinate operations
- Handle proper motions and radial velocities
- Query named objects from online databases

**See:** `references/coordinates.md` for detailed coordinate frame descriptions, transformations, observer-dependent frames (AltAz), catalog matching, and performance tips.

### 

... (truncated from original)
---
name: database-lookup
description: Search 78 public scientific, biomedical, materials science, and economic databases via REST APIs. Covers physics/astronomy (NASA, NIST, SDSS, SIMBAD), earth/environment (USGS, NOAA, EPA), chemistry/dr
triggers:
  - database lookup
  - data analysis
  - analyze data
  - statistical test
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 8
---

# Database Lookup

You have access to 78 public databases through their REST APIs. Your job is to figure out which database(s) are relevant to the user's question, query them, and return the raw JSON results along with which databases you used.

## Core Workflow

1. **Understand the query** — What is the user looking for? A compound? A gene? A pathway? A patent? Expression data? An economic indicator? This determines which database(s) to hit.

2. **Select database(s)** — Use the database selection guide below. When in doubt, search multiple databases — it's better to cast a wide net than to miss relevant data.

3. **Read the reference file** — Each database has a reference file in `references/` with endpoint details, query formats, and example calls. Read the relevant file(s) before making API calls.

4. **Make the API call(s)** — See the **Making API Calls** section below for which HTTP fetch tool to use on your platform.

5. **Return results** — Always return:
   - The **raw JSON** response from each database
   - A **list of databases queried** with the specific endpoints used
   - If a query returned no results, say so explicitly rather than omitting it

## Database Selection Guide

Match the user's intent to the right database(s). Many queries benefit from hitting multiple databases.

### Physics & Astronomy
| User is asking about... | Primary database(s) | Also consider |
|---|---|---|
| Near-Earth objects, asteroids | NASA (NeoWs) | — |
| Mars rover images | NASA (Mars Rover Photos) | — |
| Exoplanets, orbital parameters | NASA Exoplanet Archive | — |
| Astronomical objects by name/coordinates | SIMBAD | SDSS |
| Galaxy/star spectra, photometry | SDSS | SIMBAD |
| Physical constants | NIST | — |
| Atomic spectra, spectral lines | NIST (ASD) | — |

### Earth & Environmental Sciences
| User is asking about... | Primary database(s) | Also consider |
|---|---|---|
| Earthquakes, seismic events | USGS Earthquakes | — |
| Water data, streamflow, groundwater | USGS Water Services | — |
| Weather (current, forecast, historical) | OpenWeatherMap | NOAA |
| Climate data, historical weather stations | NOAA (CDO) | — |
| Air quality, toxic releases | EPA (Envirofacts) | — |

### Chemistry & Drugs
| User is asking about... | Primary database(s) | Also consider |
|---|---|---|
| Chemical compounds, molecules | PubChem | ChEMBL |
| Molecular properties (weight, formula, SMILES) | PubChem | — |
| Drug synonyms, CAS numbers | PubChem (synonyms) | DrugBank |
| Bioactivity data, IC50, binding assays | ChEMBL | BindingDB, PubChem |
| Drug binding affinities (Ki, IC50, Kd) | ChEMBL, BindingDB | PubChem |
| Drug-target interactions | ChEMBL, DrugBank | BindingDB, Open Targets |
| Ligands for a protein target (by UniProt) | BindingDB | ChEMBL |
| Target identification from compound structure | BindingDB (SMILES similarity) | ChEMBL |
| Drug labels, adverse events, recalls | FDA (OpenFDA) | DailyMed |
| Drug labels (structured product labels) | DailyMed | FDA (Op

... (truncated from original)
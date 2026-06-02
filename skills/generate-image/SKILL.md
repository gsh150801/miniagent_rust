---
name: generate-image
description: Generate or edit images using AI models (FLUX, Nano Banana 2). Use for general-purpose image generation including photos, illustrations, artwork, visual assets, concept art, and any image that is not 
triggers:
  - generate image
  - genomic
  - bioinformatics
  - sequence analysis
tools_needed:
version: "0.1.0"
author: K-Dense Inc.
priority: 6
---

# Generate Image

Generate and edit high-quality images using OpenRouter's image generation models including FLUX.2 Pro and Gemini 3.1 Flash Image Preview.

## When to Use This Skill

**Use generate-image for:**
- Photos and photorealistic images
- Artistic illustrations and artwork
- Concept art and visual concepts
- Visual assets for presentations or documents
- Image editing and modifications
- Any general-purpose image generation needs

**Use scientific-schematics instead for:**
- Flowcharts and process diagrams
- Circuit diagrams and electrical schematics
- Biological pathways and signaling cascades
- System architecture diagrams
- CONSORT diagrams and methodology flowcharts
- Any technical/schematic diagrams

## Quick Start

Use the `scripts/generate_image.py` script to generate or edit images:

```bash
# Generate a new image
python scripts/generate_image.py "A beautiful sunset over mountains"

# Edit an existing image
python scripts/generate_image.py "Make the sky purple" --input photo.jpg
```

This generates/edits an image and saves it as `generated_image.png` in the current directory.

## API Key Setup

**CRITICAL**: The script requires an OpenRouter API key. Before running, check if the user has configured their API key:

1. Look for a `.env` file in the project directory or parent directories
2. Check for `OPENROUTER_API_KEY=<key>` in the `.env` file
3. If not found, inform the user they need to:
   - Create a `.env` file with `OPENROUTER_API_KEY=your-api-key-here`
   - Or set the environment variable: `export OPENROUTER_API_KEY=your-api-key-here`
   - Get an API key from: https://openrouter.ai/keys

The script will automatically detect the `.env` file and provide clear error messages if the API key is missing.

## Model Selection

**Default model**: `google/gemini-3.1-flash-image-preview` (high quality, recommended)

**Available models for generation and editing**:
- `google/gemini-3.1-flash-image-preview` - High quality, supports generation + editing
- `black-forest-labs/flux.2-pro` - Fast, high quality, supports generation + editing

**Generation only**:
- `black-forest-labs/flux.2-flex` - Fast and cheap, but not as high quality as pro

Select based on:
- **Quality**: Use gemini-3.1-flash-image-preview or flux.2-pro
- **Editing**: Use gemini-3.1-flash-image-preview or flux.2-pro (both support image editing)
- **Cost**: Use flux.2-flex for generation only

## Common Usage Patterns

### Basic generation
```bash
python scripts/generate_image.py "Your prompt here"
```

### Specify model
```bash
python scripts/generate_image.py "A cat in space" --model "black-forest-labs/flux.2-pro"
```

### Custom output path
```bash
python scripts/generate_image.py "Abstract art" --output artwork.png
```

### Edit an existing image
```bash
python scripts/generate_image.py "Make the background blue" --input photo.jpg
```

### Edit with a specific model
```bash
python scripts/generate_image.py "Add sunglasses to the person" --input portrait.png --model "

... (truncated from original)
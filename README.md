# Kintuba AFM Viewer

A desktop application for viewing and analyzing Bruker Nanoscope SPM files (`.spm`), written in pure Rust with [egui](https://github.com/emilk/egui).

## Features

- **File browser** — Open a folder and list all Nanoscope SPM files (`.spm`, `.000`–`.009`, etc.)
- **Height map display** — Renders the AFM height channel as a color image
  - Colormaps: AFM Hot, Gray, Viridis
  - Z-range controls (drag values, extendable beyond data min/max)
  - "Data Range" button to reset to original min/max
- **Polynomial line flattening** — Orders 0–3, default order 2
- **Pan & Zoom** — Scroll wheel to zoom, drag to pan, reset button
- **Analysis tab**
  - Draw a line profile by clicking two points on the image
  - Drag the green (start) and red (end) handles to adjust
  - Cross-section plot showing distance (nm) vs. height (nm)
  - **Marker A/B** — Click on the plot to place two markers; ΔD (distance) and ΔH (height difference) are displayed prominently
  - Right-click on the plot to clear markers
  - Export profile as CSV or PNG

## Supported File Formats

| Format | Description |
|--------|-------------|
| `.spm` | Bruker Nanoscope SPM (standard extension) |
| `.000`–`.009`, `.00000`–` ` | Nanoscope files with numeric extensions |

The parser reads the text header up to `\*File list end`, selects the Height channel (Retrace preferred), converts raw LSB values to nanometers using the embedded sensitivity calibration, and optionally applies polynomial line-by-line flattening.

## Build & Run

```sh
cargo run
```

Requires Rust 2021 edition or later.

### Dependencies

| Crate | Purpose |
|-------|---------|
| `eframe` / `egui` | Native GUI framework |
| `egui_plot` | Profile plot widget |
| `rfd` | Native file dialogs |
| `image` | PNG export |

## Usage

1. Click **Open Folder** and select the directory containing your SPM files.
2. Click a file in the left panel to load it.
3. Adjust the colormap, flatten order, and Z-range as needed.
4. Switch to the **Analysis** tab to draw a line profile.
5. Click the profile plot to place markers A and B; the ΔD and ΔH values appear below the plot.
6. Use **Save CSV** or **Save PNG** to export the profile data.

## License

Copyright © Yuhei Yamada 2026. All rights reserved.

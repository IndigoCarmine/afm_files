# Kintuba AFM Viewer

A desktop application for viewing and analyzing Bruker Nanoscope SPM files (`.spm`), written in pure Rust with [egui](https://github.com/emilk/egui).

## Features

- **File browser** — Open a folder and list all Nanoscope SPM files (`.spm`, `.000`–`.009`, etc.)
  - Name filter with `_`-aware wildcards (see [File name filter](#file-name-filter))
  - Double-click a file to copy its name to the clipboard
- **Height map display** — Renders the AFM height channel as a color image
  - Colormaps: AFM Hot, Gray, Viridis
  - Z-range controls (drag values, extendable beyond data min/max)
  - "Data Range" button to reset to original min/max
- **Polynomial line flattening** — Orders 0–3, default order 2
- **Per-instrument XYZ calibration** — Correct the microscope's lateral and
  height errors automatically (see [Instrument XYZ calibration](#instrument-xyz-calibration))
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

### Instrument XYZ calibration

AFM scanners are rarely calibrated perfectly, and the residual error is specific
to the instrument: laterally it shows up as a distorted aspect ratio, and in Z as
heights that read consistently high or low. **⚙ Calibration** in the toolbar
opens a table of X/Y/Z magnification factors keyed by the `\Serial Number`
recorded in the file header (e.g. `10396jvlr`). When a file from a registered
instrument is opened, the factors are applied automatically:

- **X / Y** scale the scan extents, correcting the on-screen aspect ratio, the
  scale bar, line-profile distances, and the 3D surface.
- **Z** scales the height samples, so the Z range, profile heights, the ΔH
  readout, and exported profile CSVs all report corrected nanometres.

Because Z scales the samples *and* the Z window they are drawn through, a Z
factor never changes how the image looks — only the numbers.

**Saved and copied images keep their raw pixel grid** — resampling would put
interpolated pixels into a file meant to hold measurements. When that grid does
not carry the sample's true aspect, the app says so next to the save/copy
buttons and again after writing, naming the factor the file is missing (e.g.
"scale height ×1.0230 for the true aspect"). The burnt-in scale bar stays correct
either way, since it is measured along the image width.

Because a silently rescaled image is easy to mistake for raw data, **the frame
around the image turns red whenever a factor other than 1.0 is in effect**, and
the applied factors are shown next to it.

The table is stored as a plain text file you can also edit by hand:

| Platform | Path |
|---|---|
| Windows | `%APPDATA%\KintubaAfmViewer\calibration.txt` |
| macOS | `~/Library/Application Support/KintubaAfmViewer/calibration.txt` |
| Linux | `$XDG_CONFIG_HOME/kintuba-afm-viewer/calibration.txt` (or `~/.config/…`) |

```
# Kintuba AFM Viewer — instrument XYZ calibration
# <serial number>  <x scale>  <y scale>  <z scale>
10396jvlr  1.000000  1.023000  0.987000
```

Blank lines and `#` comments are ignored, and the `z` column may be omitted (it
defaults to 1.0). A malformed line is skipped rather than blocking startup — the
dialog reports which lines were dropped. A missing file simply means nothing is
calibrated yet (every instrument at ×1.0).

### File name filter

The box above the file list narrows it down. Matching is done against the whole
file name (extension included) and ignores case.

| Pattern | Meaning |
|---------|---------|
| `*` | any text that does **not** cross a `_` |
| `**` | any text, `_` included |
| `{<20260808}` | numeric comparison on the digits at that position — `<` `<=` `>` `>=` `=` `!=` |
| `{>=20260101,<20260808}` | comma-separated conditions are ANDed |
| `{EtOH|MeOH}` | one of the listed alternatives |

Text containing no `*` and no `{` is a plain substring search, so typing `EtOH`
is enough for a quick lookup.

```
{<20260808}_*_*_*_EtOH10_**
```

matches `20260101_sampleA_p1_run3_EtOH10_scan.003` — a date before 2026-08-08,
three arbitrary segments, the literal `EtOH10`, then anything.

## License

Copyright © Yuhei Yamada 2026. All rights reserved.

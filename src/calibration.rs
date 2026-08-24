//! Per-instrument XYZ calibration table.
//!
//! The microscope's own calibration is imperfect and the error is specific to
//! the scanner in use: laterally it shows up as a wrong aspect ratio, and in Z
//! as heights that read consistently high or low. This module stores an
//! `(x, y, z)` multiplier per instrument — keyed by the `\Serial Number`
//! recorded in the SPM header — and persists it as a human-editable text file
//! in the OS config directory:
//!
//! ```text
//! # Kintuba AFM Viewer — instrument XYZ calibration
//! # <serial number>  <x scale>  <y scale>  <z scale>
//! 10396jvlr  1.000000  1.023000  0.987000
//! ```
//!
//! Blank lines and `#` comments are ignored, and the `z` column may be omitted
//! (it defaults to 1.0) so a table written before Z calibration existed still
//! loads. A line that does not parse is skipped rather than aborting the load,
//! so a hand-edit typo never stops the app from starting; the problem is
//! reported through [`CalibrationTable::load_error`] instead.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Per-instrument scale factors: `x`/`y` multiply the lateral scan extents,
/// `z` multiplies the height samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Calibration {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Calibration {
    /// No correction at all — what an unregistered instrument gets.
    pub const UNITY: Self = Self {
        x: 1.0,
        y: 1.0,
        z: 1.0,
    };

    /// True when nothing would actually be rescaled.
    pub fn is_unity(&self) -> bool {
        const EPS: f32 = 1e-6;
        (self.x - 1.0).abs() < EPS && (self.y - 1.0).abs() < EPS && (self.z - 1.0).abs() < EPS
    }
}

impl Default for Calibration {
    fn default() -> Self {
        Self::UNITY
    }
}

/// A calibration table keyed by lower-cased instrument serial number.
#[derive(Debug, Default, Clone)]
pub struct CalibrationTable {
    entries: BTreeMap<String, Calibration>,
    load_error: Option<String>,
}

const HEADER: &str = "\
# Kintuba AFM Viewer — instrument XYZ calibration
# <serial number>  <x scale>  <y scale>  <z scale>
";

/// Directory the calibration file lives in, following each platform's
/// convention. Falls back to the current directory when the environment does
/// not expose a home/config location.
fn config_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("KintubaAfmViewer");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("KintubaAfmViewer");
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join("kintuba-afm-viewer");
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home)
                .join(".config")
                .join("kintuba-afm-viewer");
        }
    }
    PathBuf::from(".")
}

/// Normalise a serial number into a table key: trimmed, unquoted, lower-cased.
fn normalize_id(id: &str) -> String {
    id.trim().trim_matches('"').trim().to_ascii_lowercase()
}

/// Parse the file body. Returns the entries plus a description of any lines
/// that had to be skipped (`None` when everything parsed).
fn parse_table(text: &str) -> (BTreeMap<String, Calibration>, Option<String>) {
    // A factor is only meaningful if it is positive and finite; anything else
    // would produce a degenerate image or a divide-by-zero downstream.
    fn factor(tok: &str) -> Option<f32> {
        let v = tok.parse::<f32>().ok()?;
        (v.is_finite() && v > 0.0).then_some(v)
    }

    let mut entries = BTreeMap::new();
    let mut bad_lines: Vec<usize> = Vec::new();

    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        let parsed = (|| {
            // `z` is optional so tables written before Z calibration existed
            // still load; anything past it is a typo, not something to ignore.
            if parts.len() < 3 || parts.len() > 4 {
                return None;
            }
            let id = normalize_id(parts[0]);
            if id.is_empty() {
                return None;
            }
            let x = factor(parts[1])?;
            let y = factor(parts[2])?;
            let z = match parts.get(3) {
                Some(tok) => factor(tok)?,
                None => 1.0,
            };
            Some((id, Calibration { x, y, z }))
        })();

        match parsed {
            Some((id, cal)) => {
                entries.insert(id, cal);
            }
            None => bad_lines.push(i + 1),
        }
    }

    let error = (!bad_lines.is_empty()).then(|| {
        let list: Vec<String> = bad_lines.iter().map(|n| n.to_string()).collect();
        format!("Skipped lines: {}", list.join(", "))
    });
    (entries, error)
}

/// Render the table back to the on-disk text format.
fn render_table(entries: &BTreeMap<String, Calibration>) -> String {
    let mut out = String::from(HEADER);
    for (id, c) in entries {
        out.push_str(&format!("{id}  {:.6}  {:.6}  {:.6}\n", c.x, c.y, c.z));
    }
    out
}

impl CalibrationTable {
    /// Full path of the calibration file, shown in the settings dialog so the
    /// user can find and hand-edit it.
    pub fn path() -> PathBuf {
        config_dir().join("calibration.txt")
    }

    /// Read the table from disk. A missing file is not an error — it just means
    /// nothing has been calibrated yet.
    pub fn load() -> Self {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let (entries, load_error) = parse_table(&text);
                Self {
                    entries,
                    load_error,
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => Self {
                entries: BTreeMap::new(),
                load_error: Some(format!("{}: {e}", path.display())),
            },
        }
    }

    /// Write the table to disk, creating the config directory if needed.
    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "Could not create the config directory ({}): {e}",
                    parent.display()
                )
            })?;
        }
        std::fs::write(&path, render_table(&self.entries))
            .map_err(|e| format!("Could not save ({}): {e}", path.display()))
    }

    /// Calibration for an instrument; [`Calibration::UNITY`] when unregistered.
    pub fn get(&self, id: &str) -> Calibration {
        self.entries
            .get(&normalize_id(id))
            .copied()
            .unwrap_or(Calibration::UNITY)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.entries.contains_key(&normalize_id(id))
    }

    pub fn set(&mut self, id: &str, cal: Calibration) {
        let id = normalize_id(id);
        if !id.is_empty() {
            self.entries.insert(id, cal);
        }
    }

    pub fn remove(&mut self, id: &str) {
        self.entries.remove(&normalize_id(id));
    }

    /// Entries in serial-number order, for rendering the settings table.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Calibration)> {
        self.entries.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Description of lines skipped while loading, if any.
    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    /// Mutable access to one entry's factors, used by the settings dialog's
    /// `DragValue` widgets.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Calibration> {
        self.entries.get_mut(&normalize_id(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cal(x: f32, y: f32, z: f32) -> Calibration {
        Calibration { x, y, z }
    }

    #[test]
    fn unregistered_instrument_is_unity() {
        let table = CalibrationTable::default();
        assert_eq!(table.get("10396jvlr"), Calibration::UNITY);
        assert!(Calibration::UNITY.is_unity());
        assert!(!cal(1.0, 1.0, 0.987).is_unity(), "a z-only factor counts");
    }

    #[test]
    fn round_trips_through_the_text_format() {
        let mut table = CalibrationTable::default();
        table.set("10396jvlr", cal(1.0, 1.023, 0.987));
        table.set("ABC123", cal(0.98, 1.0, 1.0));

        let (entries, err) = parse_table(&render_table(&table.entries));
        assert!(err.is_none(), "clean round-trip should not report errors");

        let reloaded = CalibrationTable {
            entries,
            load_error: None,
        };
        assert_eq!(reloaded.get("10396jvlr"), cal(1.0, 1.023, 0.987));
        // Keys are stored lower-cased, so lookups are case-insensitive.
        assert_eq!(reloaded.get("abc123"), cal(0.98, 1.0, 1.0));
        assert_eq!(reloaded.get("AbC123"), cal(0.98, 1.0, 1.0));
    }

    #[test]
    fn a_table_without_a_z_column_still_loads() {
        // Tables written before Z calibration existed have three columns.
        let (entries, err) = parse_table("10396jvlr  1.0  1.023\n");
        assert!(err.is_none(), "three columns is a valid table");
        assert_eq!(entries["10396jvlr"], cal(1.0, 1.023, 1.0));
    }

    #[test]
    fn skips_comments_blanks_and_malformed_lines() {
        let text = "\
# a comment

10396jvlr  1.0  1.023  0.987
broken line
onlytwo  1.0
negative  -1.0  1.0
notanumber  x  y
zero_z  1.0  1.0  0
  9999abc   1.5   2.5  1.0  extra
";
        let (entries, err) = parse_table(text);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries["10396jvlr"], cal(1.0, 1.023, 0.987));

        let err = err.expect("malformed lines should be reported");
        // Lines 4-9 (1-indexed) are the bad ones, including the trailing junk:
        // a calibration file is not the place to quietly ignore a typo.
        for n in ["4", "5", "6", "7", "8", "9"] {
            assert!(err.contains(n), "expected line {n} in {err:?}");
        }
    }

    #[test]
    fn set_and_remove() {
        let mut table = CalibrationTable::default();
        assert!(table.is_empty());
        table.set(" 10396JVLR ", cal(1.1, 1.2, 1.3));
        assert!(table.contains("10396jvlr"));
        assert_eq!(table.get("10396jvlr"), cal(1.1, 1.2, 1.3));
        table.remove("10396JVLR");
        assert!(table.is_empty());
    }
}

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CONFIG_FILE: &str = "config.toml";
pub const IMAGES_DIR: &str = "input_images";

const DEFAULT_IMAGES: &[(&str, &[u8])] = &[
    ("1_button.png", include_bytes!("../input_images/1_button.png")),
    ("2_button.png", include_bytes!("../input_images/2_button.png")),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub max_loops: u32,
    pub click_interval_ms: u32,
    pub loop_interval_ms: u32,
    pub threshold: f32,
    pub minimize_on_start: bool,
    /// File names inside the images folder that should be skipped.
    pub disabled_images: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_loops: 99999,
            click_interval_ms: 1000,
            loop_interval_ms: 3000,
            threshold: 0.8,
            minimize_on_start: false,
            disabled_images: Vec::new(),
        }
    }
}

impl Config {
    pub fn load(base: &Path) -> Self {
        std::fs::read_to_string(base.join(CONFIG_FILE))
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, base: &Path) -> Result<()> {
        std::fs::write(base.join(CONFIG_FILE), toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// Folder holding `config.toml` and `input_images/`: next to the executable,
/// unless the current directory already has an images folder (e.g. `cargo run`).
pub fn base_dir() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let cwd = std::env::current_dir().ok();
    for d in [&exe_dir, &cwd].into_iter().flatten() {
        if d.join(IMAGES_DIR).is_dir() {
            return d.clone();
        }
    }
    exe_dir.or(cwd).unwrap_or_else(|| PathBuf::from("."))
}

/// Creates the images folder with the bundled Nexus buttons if it is missing.
pub fn ensure_images_dir(dir: &Path) -> Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    for (name, bytes) in DEFAULT_IMAGES {
        std::fs::write(dir.join(name), bytes)?;
    }
    Ok(())
}

/// All `.png` files in the images folder, sorted by name (like `glob` order).
pub fn list_images(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.is_file()
                        && p.extension()
                            .is_some_and(|e| e.eq_ignore_ascii_case("png"))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

//! Preferences that outlive the process: size, type, opacity and frame.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const MIN_FONT: f32 = 10.0;
pub const MAX_FONT: f32 = 120.0;
pub const MIN_OPACITY: f32 = 0.15;
pub const MAX_OPACITY: f32 = 1.0;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Family name. `None` means the system UI font, which on macOS is SF Pro.
    pub font_family: Option<String>,
    pub font_size: f32,
    /// 100 to 900, as CSS weights. Variable fonts follow it, static ones snap
    /// to the nearest installed face.
    pub font_weight: f32,
    /// Backdrop alpha, 0.15 to 1.0. Text stays opaque whatever this is.
    pub opacity: f32,
    pub width: f32,
    pub height: f32,
    pub x: Option<f32>,
    pub y: Option<f32>,
    /// Last script opened, reopened on launch when it still exists.
    pub script_path: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_family: None,
            font_size: 34.0,
            font_weight: 400.0,
            opacity: 0.88,
            width: 660.0,
            height: 264.0,
            x: None,
            y: None,
            script_path: None,
        }
    }
}

impl Settings {
    pub fn clamp(&mut self) {
        self.font_size = self.font_size.clamp(MIN_FONT, MAX_FONT);
        self.font_weight = self.font_weight.clamp(100.0, 900.0);
        self.opacity = self.opacity.clamp(MIN_OPACITY, MAX_OPACITY);
        self.width = self.width.clamp(320.0, 4000.0);
        self.height = self.height.clamp(140.0, 3000.0);
        if self.x.is_some_and(|x| !x.is_finite()) {
            self.x = None;
        }
        if self.y.is_some_and(|y| !y.is_finite()) {
            self.y = None;
        }
    }

    pub fn load() -> Self {
        let path = config_file();
        let mut settings = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Settings>(&text).ok())
            .unwrap_or_default();
        settings.clamp();
        settings
    }

    pub fn to_json(&self) -> Option<String> {
        serde_json::to_string_pretty(self).ok()
    }

    pub fn write_serialized(text: &str) {
        let path = config_file();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, text);
    }

    #[allow(dead_code)]
    pub fn save(&self) {
        if let Some(text) = self.to_json() {
            Self::write_serialized(&text);
        }
    }
}

fn config_file() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"));
    #[cfg(target_os = "macos")]
    let dir = home.join("Library").join("Application Support").join("Prompter");
    #[cfg(not(target_os = "macos"))]
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"))
        .join("prompter");
    dir.join("settings.json")
}

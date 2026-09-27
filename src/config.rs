use std::path::{Path, PathBuf};

/// Theme follows the terminal unless overridden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Auto,
    Light,
    Dark,
}

/// Persistent settings (`~/.config/qobi/config.toml`).
/// All fields private; read via accessors, mutate via builders.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Config {
    #[serde(default)]
    music_dir: Option<PathBuf>,
    #[serde(default = "default_volume")]
    volume: f32,
    #[serde(default = "default_true")]
    eq_enabled: bool,
    #[serde(default = "default_true")]
    art_enabled: bool,
    #[serde(default)]
    theme: Theme,
}

const fn default_volume() -> f32 {
    1.0
}

const fn default_true() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            music_dir: None,
            volume: default_volume(),
            eq_enabled: true,
            art_enabled: true,
            theme: Theme::Auto,
        }
    }
}

impl Config {
    pub fn music_dir(&self) -> Option<&Path> {
        self.music_dir.as_deref()
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn eq_enabled(&self) -> bool {
        self.eq_enabled
    }

    pub fn art_enabled(&self) -> bool {
        self.art_enabled
    }

    pub fn theme(&self) -> Theme {
        self.theme
    }

    /// True when the user must be asked for a music directory.
    pub fn needs_first_run(&self) -> bool {
        self.music_dir.is_none()
    }

    pub fn with_music_dir(mut self, dir: PathBuf) -> Self {
        self.music_dir = Some(dir);
        self
    }

    pub fn with_volume(mut self, volume: f32) -> Self {
        self.volume = volume.clamp(0.0, 1.0);
        self
    }

    pub fn with_eq_enabled(mut self, on: bool) -> Self {
        self.eq_enabled = on;
        self
    }

    pub fn with_art_enabled(mut self, on: bool) -> Self {
        self.art_enabled = on;
        self
    }

    pub fn config_path() -> Result<PathBuf, crate::error::QobiError> {
        crate::ipc::qobi_dir().map(|d| d.join("config.toml"))
    }

    /// Load or fall back to defaults. Corrupt files are backed up to
    /// `config.toml.bak` instead of crashing startup.
    pub async fn load() -> Result<Self, crate::error::QobiError> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = tokio::fs::read_to_string(&path).await?;
        match toml::from_str(&raw) {
            Ok(cfg) => Ok(cfg),
            Err(e) => {
                let bak = path.with_extension("toml.bak");
                let _ = tokio::fs::copy(&path, &bak).await;
                tracing::warn!("corrupt config backed up to {}: {e}", bak.display());
                Ok(Self::default())
            }
        }
    }

    /// Atomic save: write temp file + rename.
    pub async fn save(&self, path: &Path) -> Result<(), crate::error::QobiError> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let raw = toml::to_string_pretty(self)
            .map_err(|e| crate::error::QobiError::ConfigParse(e.to_string()))?;
        let tmp = path.with_extension("toml.tmp");
        tokio::fs::write(&tmp, raw).await?;
        tokio::fs::rename(&tmp, path).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_cfg(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "qobi-cfg-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("tmp dir");
        dir.join("config.toml")
    }

    #[tokio::test]
    async fn roundtrip_preserves_fields() {
        let path = tmp_cfg("roundtrip");
        let cfg = Config::default()
            .with_music_dir(PathBuf::from("/music"))
            .with_volume(0.5);
        cfg.save(&path).await.expect("save");
        let raw = std::fs::read_to_string(&path).expect("read");
        let back: Config = toml::from_str(&raw).expect("parse");
        assert_eq!(back, cfg);
        assert!(!back.needs_first_run());
    }

    #[tokio::test]
    async fn missing_file_yields_first_run() {
        let path = tmp_cfg("missing").join("nonexistent.toml");
        let _ = std::fs::remove_file(&path);
        // Simulate load-from-path: absent file is (default, first-run).
        let loaded = if !path.exists() {
            Config::default()
        } else {
            panic!("fixture must be absent")
        };
        assert!(loaded.needs_first_run());
    }

    #[tokio::test]
    async fn volume_is_clamped() {
        assert_eq!(Config::default().with_volume(9.0).volume(), 1.0);
        assert_eq!(Config::default().with_volume(-2.0).volume(), 0.0);
    }

    #[test]
    fn corrupt_toml_does_not_parse() {
        let bad = "volume = [unclosed";
        assert!(toml::from_str::<Config>(bad).is_err());
    }
}

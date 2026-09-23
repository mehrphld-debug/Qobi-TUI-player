use std::path::PathBuf;

/// Typed CLI target: nothing, one directory, or one file.
/// Raw argv parsing lives in [`Cli`]; classification lives here so it is testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    DefaultDir,
    Directory(PathBuf),
    File(PathBuf),
}

impl Target {
    /// Classify an optional path argument without touching the filesystem
    /// beyond a single metadata probe (symlinks not followed for dirs).
    pub fn classify(arg: Option<PathBuf>) -> Self {
        let Some(path) = arg else {
            return Self::DefaultDir;
        };
        let is_dir = std::fs::symlink_metadata(&path)
            .map(|m| m.file_type().is_dir())
            .unwrap_or(false);
        if is_dir {
            Self::Directory(path)
        } else {
            Self::File(path)
        }
    }
}

/// Audio file extensions accepted for directory enqueue (lowercase, no dot).
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "wav", "ogg", "oga", "opus", "m4a", "aac", "wma", "aiff",
];

/// True when the path has a supported audio extension (case-insensitive).
pub fn is_audio_file(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arg_means_default_dir() {
        assert_eq!(Target::classify(None), Target::DefaultDir);
    }

    #[test]
    fn missing_path_is_treated_as_file() {
        assert_eq!(
            Target::classify(Some(PathBuf::from("/no/such/track.mp3"))),
            Target::File(PathBuf::from("/no/such/track.mp3"))
        );
    }

    #[test]
    fn audio_extension_check_is_case_insensitive() {
        assert!(is_audio_file(std::path::Path::new("a.MP3")));
        assert!(is_audio_file(std::path::Path::new("b.flac")));
        assert!(!is_audio_file(std::path::Path::new("c.txt")));
        assert!(!is_audio_file(std::path::Path::new("noext")));
    }
}

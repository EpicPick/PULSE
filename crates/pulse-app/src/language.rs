use pulse_ui::{Language, LanguagePreference};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

fn settings_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(path).join("PULSE").join("language"))
}

pub fn system() -> Language {
    #[cfg(windows)]
    if pulse_platform_windows::ui_language_is_french() {
        return Language::French;
    }
    Language::English
}

pub fn load() -> LanguagePreference {
    settings_path().map_or_else(LanguagePreference::default, |path| read(&path))
}

pub fn save(preference: LanguagePreference) -> bool {
    settings_path().is_some_and(|path| write(&path, preference).is_ok())
}

fn read(path: &Path) -> LanguagePreference {
    let mut code = String::new();
    if std::fs::File::open(path)
        .and_then(|file| file.take(17).read_to_string(&mut code))
        .is_err()
        || code.len() > 16
    {
        LanguagePreference::System
    } else {
        LanguagePreference::from_code(&code)
    }
}

fn write(path: &Path, preference: LanguagePreference) -> io::Result<()> {
    let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    std::fs::create_dir_all(parent)?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = parent.join(format!(".language-{}-{nonce}", std::process::id()));
    let mut file = std::fs::File::create_new(&tmp)?;
    let result = file.write_all(preference.code().as_bytes());
    drop(file);
    let result = result.and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_invalid_file_falls_back_to_system() {
        let dir = std::env::temp_dir().join(format!("pulse-language-test-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("language");
        assert_eq!(read(&path), LanguagePreference::System);
        for value in [
            LanguagePreference::French,
            LanguagePreference::English,
            LanguagePreference::System,
        ] {
            write(&path, value).unwrap();
            assert_eq!(read(&path), value);
        }
        std::fs::write(&path, "invalid preference").unwrap();
        assert_eq!(read(&path), LanguagePreference::System);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}

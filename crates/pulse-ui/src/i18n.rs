use std::{borrow::Cow, cell::Cell, collections::HashMap, sync::OnceLock};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LanguagePreference {
    #[default]
    System,
    French,
    English,
}

impl LanguagePreference {
    pub fn from_code(code: &str) -> Self {
        match code.trim() {
            "fr" => Self::French,
            "en" => Self::English,
            _ => Self::System,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::French => "fr",
            Self::English => "en",
        }
    }

    pub fn resolve(self, system: Language) -> Language {
        match self {
            Self::System => system,
            Self::French => Language::French,
            Self::English => Language::English,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    French,
    English,
}

thread_local! { static ACTIVE: Cell<Language> = const { Cell::new(Language::French) }; }

pub fn with_language<T>(language: Language, f: impl FnOnce() -> T) -> T {
    struct Restore(Language);
    impl Drop for Restore {
        fn drop(&mut self) {
            ACTIVE.set(self.0);
        }
    }
    let _restore = Restore(ACTIVE.replace(language));
    f()
}

pub(crate) fn english() -> bool {
    ACTIVE.get() == Language::English
}

// Choose before formatting so punctuation, units and word order are translatable,
// without ever interpreting a process name or a path as a translation key.
macro_rules! trf {
    (($fr:literal, $en:literal) $($args:tt)*) => {
        if $crate::i18n::english() { format!($en $($args)*) } else { format!($fr $($args)*) }
    };
}
pub(crate) use trf;

fn catalog() -> &'static HashMap<&'static str, &'static str> {
    static CATALOG: OnceLock<HashMap<&str, &str>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        include_str!("messages.txt")
            .lines()
            .filter_map(|line| line.split_once(" => "))
            .collect()
    })
}

pub(crate) fn tr(text: &str) -> &str {
    if english() {
        catalog().get(text).copied().unwrap_or(text)
    } else {
        text
    }
}

/// Translate known application messages. Leave Windows/library error details intact.
pub fn message(text: &str) -> Cow<'_, str> {
    if english() {
        if let Some(english) = catalog().get(text) {
            return Cow::Borrowed(english);
        }
        if let Some((prefix, detail)) = text.split_once(": ")
            && let Some(english) = catalog().get(prefix.trim_end())
        {
            return Cow::Owned(format!("{english}: {}", tr(detail)));
        }
        if let Some(max) = text.strip_prefix("Rétention hors de l’intervalle 1..=")
            && max.parse::<usize>().is_ok()
        {
            return Cow::Owned(format!("Retention must be between 1 and {max}"));
        }
        if let Some(detail) = text.strip_prefix("Windows a refusé la demande (") {
            return Cow::Owned(format!("Windows denied the request ({detail}"));
        }
        if let Some(detail) =
            text.strip_prefix("PresentMon n’a pas produit de capture exploitable (")
        {
            let detail = detail.replace(" Vérifiez le CLI 2.x et les droits ETW du compte (Utilisateurs des journaux de performances). PULSE ne s’élève pas.", " Check the 2.x CLI and this account’s ETW permissions (Performance Log Users). PULSE does not elevate.");
            return Cow::Owned(format!(
                "PresentMon did not produce a usable capture ({detail}"
            ));
        }
    }
    Cow::Borrowed(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_unique_keys_and_preserves_external_details() {
        let lines = include_str!("messages.txt").lines();
        assert!(lines.clone().all(|line| {
            line.split_once(" => ")
                .is_some_and(|(fr, en)| !fr.is_empty() && !en.is_empty())
        }));
        assert_eq!(lines.count(), catalog().len());
        with_language(Language::English, || {
            assert_eq!(
                message("Impossible de lire la session : C:\\Sessions\\test.sqlite"),
                "Could not read the session: C:\\Sessions\\test.sqlite"
            );
            assert_eq!(message("Accès refusé"), "Access denied");
            with_language(Language::French, || {
                assert_eq!(message("Accès refusé"), "Accès refusé")
            });
            assert_eq!(message("Accès refusé"), "Access denied");
        });
    }
}

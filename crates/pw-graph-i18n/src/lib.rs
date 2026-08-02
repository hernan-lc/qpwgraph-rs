//! Small catalog-based localization layer with English fallback.

use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
pub enum Locale {
    #[default]
    English,
    Spanish,
    French,
}

impl Locale {
    pub const ALL: [Self; 3] = [Self::English, Self::Spanish, Self::French];

    pub fn parse(value: &str) -> Self {
        let language = value.trim().to_ascii_lowercase();
        if language == "es" || language.starts_with("es-") || language.starts_with("es_") {
            Self::Spanish
        } else if language == "fr" || language.starts_with("fr-") || language.starts_with("fr_") {
            Self::French
        } else {
            Self::English
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Spanish => "es",
            Self::French => "fr",
        }
    }

    pub fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Spanish => "Español",
            Self::French => "Français",
        }
    }
}

#[derive(Clone, Debug)]
pub struct I18n {
    locale: Locale,
    english: BTreeMap<String, String>,
    current: BTreeMap<String, String>,
}

impl Default for I18n {
    fn default() -> Self {
        Self::new(Locale::default())
    }
}

impl I18n {
    pub fn new(locale: Locale) -> Self {
        let english = load_catalog(include_str!("../locales/en.json"));
        let current = if locale == Locale::English {
            english.clone()
        } else {
            load_catalog(match locale {
                Locale::Spanish => include_str!("../locales/es.json"),
                Locale::French => include_str!("../locales/fr.json"),
                Locale::English => unreachable!(),
            })
        };
        Self {
            locale,
            english,
            current,
        }
    }

    pub fn from_language(value: &str) -> Self {
        Self::new(Locale::parse(value))
    }

    pub fn locale(&self) -> Locale {
        self.locale
    }

    pub fn set_locale(&mut self, locale: Locale) {
        self.locale = locale;
        self.current = if locale == Locale::English {
            self.english.clone()
        } else {
            load_catalog(match locale {
                Locale::Spanish => include_str!("../locales/es.json"),
                Locale::French => include_str!("../locales/fr.json"),
                Locale::English => unreachable!(),
            })
        };
    }

    pub fn text(&self, key: &str) -> String {
        self.current
            .get(key)
            .or_else(|| self.english.get(key))
            .cloned()
            .unwrap_or_else(|| key.to_owned())
    }

    pub fn format(&self, key: &str, variables: &[(&str, String)]) -> String {
        let mut message = self.text(key);
        for (name, value) in variables {
            message = message.replace(&format!("{{{name}}}"), value);
        }
        message
    }
}

fn load_catalog(text: &str) -> BTreeMap<String, String> {
    serde_json::from_str(text).expect("bundled locale catalog must be valid JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spanish_translates_and_falls_back() {
        let i18n = I18n::new(Locale::Spanish);
        assert_eq!(i18n.text("toolbar.undo"), "Deshacer");
        assert_eq!(i18n.text("missing.key"), "missing.key");
    }

    #[test]
    fn formatting_replaces_named_variables() {
        let i18n = I18n::default();
        assert_eq!(
            i18n.format(
                "status.connected",
                &[("output", "1".into()), ("input", "2".into())]
            ),
            "Connected port 1 to 2"
        );
    }

    #[test]
    fn locale_catalogs_cover_the_same_keys() {
        let english = load_catalog(include_str!("../locales/en.json"));
        let spanish = load_catalog(include_str!("../locales/es.json"));
        assert_eq!(
            english.keys().collect::<Vec<_>>(),
            spanish.keys().collect::<Vec<_>>()
        );
    }
}

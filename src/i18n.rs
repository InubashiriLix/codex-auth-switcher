//! Embedded user-interface translations and locale selection.

use fluent_bundle::{FluentArgs, FluentResource, concurrent::FluentBundle};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    env,
    sync::OnceLock,
};
use unic_langid::LanguageIdentifier;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LanguagePreference {
    #[default]
    Auto,
    ZhCn,
    En,
    Ja,
    Es,
    De,
    It,
    Fr,
}

impl LanguagePreference {
    pub const ALL: [Self; 8] = [
        Self::Auto,
        Self::ZhCn,
        Self::En,
        Self::Ja,
        Self::Es,
        Self::De,
        Self::It,
        Self::Fr,
    ];

    pub fn resolve(self) -> Language {
        match self {
            Self::Auto => Language::from_environment(),
            Self::ZhCn => Language::ZhCn,
            Self::En => Language::En,
            Self::Ja => Language::Ja,
            Self::Es => Language::Es,
            Self::De => Language::De,
            Self::It => Language::It,
            Self::Fr => Language::Fr,
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::ZhCn => "zh-cn",
            Self::En => "en",
            Self::Ja => "ja",
            Self::Es => "es",
            Self::De => "de",
            Self::It => "it",
            Self::Fr => "fr",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    ZhCn,
    En,
    Ja,
    Es,
    De,
    It,
    Fr,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LocalizedMessage {
    pub key: String,
    #[serde(default)]
    pub args: BTreeMap<String, String>,
}

impl LocalizedMessage {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            args: BTreeMap::new(),
        }
    }

    pub fn render(&self, language: Language, fallback: &str) -> String {
        let values = self
            .args
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()));
        let rendered = translate_with(language, &self.key, values);
        if rendered == self.key {
            fallback.to_owned()
        } else {
            rendered
        }
    }
}

impl Language {
    pub const ALL: [Self; 7] = [
        Self::ZhCn,
        Self::En,
        Self::Ja,
        Self::Es,
        Self::De,
        Self::It,
        Self::Fr,
    ];

    pub fn from_environment() -> Self {
        for name in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(value) = env::var(name)
                && !value.trim().is_empty()
            {
                return Self::from_locale_str(&value);
            }
        }
        Self::En
    }

    pub fn from_locale_str(value: &str) -> Self {
        let normalized = value.trim().to_ascii_lowercase().replace('_', "-");
        let code = normalized.split(['.', '@']).next().unwrap_or(&normalized);
        match code.split('-').next().unwrap_or(code) {
            "zh" => Self::ZhCn,
            "ja" => Self::Ja,
            "es" => Self::Es,
            "de" => Self::De,
            "it" => Self::It,
            "fr" => Self::Fr,
            "en" => Self::En,
            _ => Self::En,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::ZhCn => "zh-CN",
            Self::En => "en",
            Self::Ja => "ja",
            Self::Es => "es",
            Self::De => "de",
            Self::It => "it",
            Self::Fr => "fr",
        }
    }

    pub fn native_name(self) -> &'static str {
        match self {
            Self::ZhCn => "中文",
            Self::En => "English",
            Self::Ja => "日本語",
            Self::Es => "Español",
            Self::De => "Deutsch",
            Self::It => "Italiano",
            Self::Fr => "Français",
        }
    }
}

type Bundle = FluentBundle<FluentResource>;
static BUNDLES: OnceLock<HashMap<Language, Bundle>> = OnceLock::new();
const CATALOGS: [(Language, &str); 7] = [
    (Language::ZhCn, include_str!("../locales/zh-CN.ftl")),
    (Language::En, include_str!("../locales/en.ftl")),
    (Language::Ja, include_str!("../locales/ja.ftl")),
    (Language::Es, include_str!("../locales/es.ftl")),
    (Language::De, include_str!("../locales/de.ftl")),
    (Language::It, include_str!("../locales/it.ftl")),
    (Language::Fr, include_str!("../locales/fr.ftl")),
];

fn bundles() -> &'static HashMap<Language, Bundle> {
    BUNDLES.get_or_init(|| {
        CATALOGS
            .into_iter()
            .map(|(language, source)| {
                let locale: LanguageIdentifier =
                    language.code().parse().expect("valid embedded locale");
                let mut bundle = FluentBundle::new_concurrent(vec![locale]);
                let resource = FluentResource::try_new(source.to_owned())
                    .expect("valid embedded Fluent resource");
                bundle
                    .add_resource(resource)
                    .expect("unique embedded Fluent messages");
                (language, bundle)
            })
            .collect()
    })
}

pub fn translate(language: Language, id: &str, args: Option<&FluentArgs<'_>>) -> String {
    fn lookup(bundle: &Bundle, id: &str, args: Option<&FluentArgs<'_>>) -> Option<String> {
        let pattern = bundle.get_message(id)?.value()?;
        let mut errors = Vec::new();
        Some(
            bundle
                .format_pattern(pattern, args, &mut errors)
                .into_owned(),
        )
    }
    lookup(&bundles()[&language], id, args)
        .or_else(|| lookup(&bundles()[&Language::En], id, args))
        .unwrap_or_else(|| id.to_owned())
}

pub fn translate_with<'a, I, K, V>(language: Language, id: &str, values: I) -> String
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<std::borrow::Cow<'a, str>>,
    V: Into<fluent_bundle::FluentValue<'a>>,
{
    let mut args = FluentArgs::new();
    for (key, value) in values {
        args.set(key, value);
    }
    translate(language, id, Some(&args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_has_a_working_catalog() {
        for language in Language::ALL {
            assert_ne!(
                translate(language, "language-title", None),
                "language-title"
            );
            assert!(!translate(language, "ready", None).is_empty());
        }
    }

    #[test]
    fn catalogs_have_identical_message_keys() {
        fn keys(source: &str) -> Vec<&str> {
            let mut keys = source
                .lines()
                .filter(|line| !line.starts_with(char::is_whitespace) && !line.starts_with('#'))
                .filter_map(|line| line.split_once('=').map(|(key, _)| key.trim()))
                .collect::<Vec<_>>();
            keys.sort_unstable();
            keys
        }
        let expected = keys(CATALOGS[1].1);
        for (language, source) in CATALOGS {
            assert_eq!(keys(source), expected, "catalog mismatch for {language:?}");
        }
    }

    #[test]
    fn catalogs_have_identical_message_variables() {
        fn variables(source: &str) -> BTreeMap<&str, Vec<&str>> {
            source
                .lines()
                .filter(|line| !line.starts_with(char::is_whitespace) && !line.starts_with('#'))
                .filter_map(|line| line.split_once('='))
                .map(|(key, value)| {
                    let mut variables = value
                        .match_indices('$')
                        .map(|(index, _)| {
                            value[index + 1..]
                                .split(|character: char| {
                                    !character.is_ascii_alphanumeric() && character != '-'
                                })
                                .next()
                                .unwrap_or("")
                        })
                        .filter(|variable| !variable.is_empty())
                        .collect::<Vec<_>>();
                    variables.sort_unstable();
                    (key.trim(), variables)
                })
                .collect()
        }
        let expected = variables(CATALOGS[1].1);
        for (language, source) in CATALOGS {
            assert_eq!(
                variables(source),
                expected,
                "variable mismatch for {language:?}"
            );
        }
    }

    #[test]
    fn preferences_have_stable_config_values() {
        assert_eq!(LanguagePreference::ALL.len(), 8);
        assert_eq!(LanguagePreference::ZhCn.config_value(), "zh-cn");
    }

    #[test]
    fn locale_variants_are_normalized_and_unknown_values_fall_back_to_english() {
        assert_eq!(Language::from_locale_str("zh_CN.UTF-8"), Language::ZhCn);
        assert_eq!(Language::from_locale_str("ja-JP"), Language::Ja);
        assert_eq!(Language::from_locale_str("pt_BR.UTF-8"), Language::En);
        assert_eq!(Language::from_locale_str("C"), Language::En);
    }
}

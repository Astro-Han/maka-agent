/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

use fluent_bundle::{FluentArgs, FluentResource, concurrent::FluentBundle};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    str::FromStr,
    sync::{Arc, OnceLock},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Locale {
    En,
    ZhCn,
    ZhTw,
}

impl Locale {
    pub const ALL: [Self; 3] = [Self::En, Self::ZhCn, Self::ZhTw];
    pub fn id(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::ZhCn => "zh-CN",
            Self::ZhTw => "zh-TW",
        }
    }
    pub fn native_name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::ZhCn => "简体中文",
            Self::ZhTw => "繁體中文",
        }
    }
    fn index(self) -> usize {
        match self {
            Self::En => 0,
            Self::ZhCn => 1,
            Self::ZhTw => 2,
        }
    }
    fn from_system(value: &str) -> Self {
        let normalized = value.trim().replace('_', "-").to_ascii_lowercase();
        let parts: Vec<_> = normalized.split(['-', '.']).collect();
        if parts.first() != Some(&"zh") {
            return Self::En;
        }
        if matches!(parts.get(1), Some(&("tw" | "hk" | "mo" | "hant"))) {
            Self::ZhTw
        } else {
            Self::ZhCn
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LocalePreference {
    #[default]
    Auto,
    Explicit(Locale),
}
impl FromStr for LocalePreference {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().replace('_', "-").to_ascii_lowercase().as_str() {
            "" | "auto" => Ok(Self::Auto),
            "en" => Ok(Self::Explicit(Locale::En)),
            "zh" | "zh-cn" => Ok(Self::Explicit(Locale::ZhCn)),
            "zh-tw" => Ok(Self::Explicit(Locale::ZhTw)),
            _ => Err(format!(
                "Invalid locale {value:?}. Expected zh-CN, zh-TW, en, or auto."
            )),
        }
    }
}

/// Same authority rules as packages/cli/src/cli-ui-locale.ts.
/// LC_ALL, LC_MESSAGES and LANG are overrides, not a language preference list.
fn resolve(
    explicit: Option<LocalePreference>,
    maka_locale: Option<&str>,
    system: [Option<&str>; 3],
    native: Option<&str>,
) -> Result<(LocalePreference, Locale), String> {
    let preference = explicit
        .map(Ok)
        .unwrap_or_else(|| maka_locale.unwrap_or("auto").parse())?;
    let system = system
        .into_iter()
        .flatten()
        .find(|s| !s.trim().is_empty())
        .or(native);
    Ok((preference, Locale::from_system(system.unwrap_or("en"))))
}

type Catalogs = [FluentBundle<FluentResource>; 3];
const SOURCES: [&str; 3] = [
    include_str!("../locales/en.ftl"),
    include_str!("../locales/zh-CN.ftl"),
    include_str!("../locales/zh-TW.ftl"),
];

fn catalogs(sources: [&str; 3]) -> Catalogs {
    std::array::from_fn(|index| {
        let locale: unic_langid::LanguageIdentifier = Locale::ALL[index].id().parse().unwrap();
        let mut bundle = FluentBundle::new_concurrent(vec![locale]);
        // All shipped languages are LTR. Invisible bidi isolates confuse terminal
        // cell layout; revisit together with RTL layout when adding an RTL locale.
        bundle.set_use_isolating(false);
        bundle
            .add_resource(
                FluentResource::try_new(sources[index].into())
                    .expect("embedded TUI locale must parse"),
            )
            .expect("embedded TUI message IDs must be unique");
        bundle
    })
}

pub struct I18n {
    pub preference: LocalePreference,
    system: Locale,
    catalogs: Arc<Catalogs>,
    // Bounded, deduplicated diagnostics: never print into the alternate screen.
    diagnostics: RefCell<BTreeSet<String>>,
}
impl I18n {
    pub fn from_environment(explicit: Option<LocalePreference>) -> Result<Self, String> {
        let environment =
            ["MAKA_LOCALE", "LC_ALL", "LC_MESSAGES", "LANG"].map(|key| std::env::var(key).ok());
        let native = sys_locale::get_locale();
        let (preference, system) = resolve(
            explicit,
            environment[0].as_deref(),
            [
                environment[1].as_deref(),
                environment[2].as_deref(),
                environment[3].as_deref(),
            ],
            native.as_deref(),
        )?;
        Ok(Self::new(preference, system))
    }
    pub fn new(preference: LocalePreference, system: Locale) -> Self {
        static CATALOGS: OnceLock<Arc<Catalogs>> = OnceLock::new();
        Self {
            preference,
            system,
            catalogs: CATALOGS.get_or_init(|| Arc::new(catalogs(SOURCES))).clone(),
            diagnostics: RefCell::default(),
        }
    }
    pub fn locale(&self) -> Locale {
        match self.preference {
            LocalePreference::Auto => self.system,
            LocalePreference::Explicit(locale) => locale,
        }
    }
    /// The environment's locale, which `Auto` follows.
    pub fn system(&self) -> Locale {
        self.system
    }
    pub fn cycle(&mut self) {
        self.preference = match self.preference {
            LocalePreference::Auto => LocalePreference::Explicit(Locale::ZhCn),
            LocalePreference::Explicit(Locale::ZhCn) => LocalePreference::Explicit(Locale::ZhTw),
            LocalePreference::Explicit(Locale::ZhTw) => LocalePreference::Explicit(Locale::En),
            LocalePreference::Explicit(Locale::En) => LocalePreference::Auto,
        };
    }
    pub fn language_name(&self) -> String {
        match self.preference {
            LocalePreference::Auto => {
                self.format("language-auto", &[("language", self.system.native_name())])
            }
            LocalePreference::Explicit(locale) => locale.native_name().into(),
        }
    }
    pub fn text(&self, key: &str) -> String {
        self.format(key, &[])
    }
    pub fn format(&self, key: &str, values: &[(&str, &str)]) -> String {
        let mut args = FluentArgs::new();
        for (name, value) in values {
            args.set(*name, *value);
        }
        let locale = self.locale();
        self.try_format(locale, key, &args)
            .or_else(|| {
                (locale != Locale::En)
                    .then(|| self.try_format(Locale::En, key, &args))
                    .flatten()
            })
            .unwrap_or_else(|| format!("[{key}]"))
    }
    fn try_format(&self, locale: Locale, key: &str, args: &FluentArgs<'_>) -> Option<String> {
        let bundle = &self.catalogs[locale.index()];
        let pattern = bundle.get_message(key).and_then(|message| message.value());
        if let Some(pattern) = pattern {
            let mut errors = vec![];
            let text = bundle.format_pattern(pattern, Some(args), &mut errors);
            if errors.is_empty() {
                return Some(text.into_owned());
            }
            self.record(format!("{}/{key}: {errors:?}", locale.id()));
        } else {
            self.record(format!("{}/{key}: missing message", locale.id()));
        }
        None
    }
    fn record(&self, diagnostic: String) {
        let mut diagnostics = self.diagnostics.borrow_mut();
        if diagnostics.len() < 32 {
            diagnostics.insert(diagnostic);
        }
    }
    pub fn diagnostics(&self) -> Vec<String> {
        self.diagnostics.borrow().iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_precedence_matches_cli_authorities_and_chinese_variants() {
        for (system, expected) in [
            ([Some("C"), Some("zh_CN.UTF-8"), Some("zh_TW")], Locale::En),
            ([Some(""), Some("zh_HK.UTF-8"), Some("en_US")], Locale::ZhTw),
            ([None, None, Some("zh-Hant")], Locale::ZhTw),
            ([None, None, Some("zh_SG.UTF-8")], Locale::ZhCn),
            ([Some("fr_FR"), None, Some("zh_CN")], Locale::En),
        ] {
            assert_eq!(
                resolve(None, None, system, Some("zh_TW")).unwrap(),
                (LocalePreference::Auto, expected)
            );
        }
        assert_eq!(
            resolve(None, Some("zh"), [None; 3], None).unwrap().0,
            LocalePreference::Explicit(Locale::ZhCn)
        );
        assert!(resolve(None, Some("invalid"), [None; 3], None).is_err());
        assert_eq!(
            resolve(
                Some(LocalePreference::Auto),
                Some("invalid"),
                [None; 3],
                Some("zh_TW")
            )
            .unwrap(),
            (LocalePreference::Auto, Locale::ZhTw)
        );
    }

    #[test]
    fn shipped_catalogs_have_matching_keys_and_parameters_and_format_without_fallback() {
        fn messages(source: &str) -> BTreeSet<&str> {
            source
                .lines()
                .filter(|line| !line.starts_with([' ', '#']))
                .filter_map(|line| line.split_once(" = ").map(|(id, _)| id))
                .collect()
        }
        let keys = messages(SOURCES[0]);
        let values = [
            ("path", "/tmp/中文"),
            ("value", "42"),
            ("language", "繁體中文"),
            ("count", "2"),
            ("kind", "session.catalog"),
            ("revision", "19"),
            ("id", "message-test"),
            ("session", "session-test"),
            ("turn", "turn-test"),
            ("run", "run-test"),
            ("error", "synthetic diagnostic"),
            ("field", "Field 中文"),
            ("min", "1"),
            ("max", "3"),
            ("index", "1"),
            ("name", "Fixture"),
            ("source", "test source"),
            ("state", "Result received"),
            ("action", "Expand message"),
            ("summary", "Read × 2"),
            ("strategy", "exact match"),
            ("bytes", "17"),
            ("start", "1"),
            ("end", "2"),
            ("total", "3"),
            ("input", "9500"),
            ("window", "128000"),
            ("code", "7"),
            ("level", "High"),
            ("line", "2"),
            ("column", "5"),
            ("setting", "Palette"),
        ];
        for locale in Locale::ALL {
            assert_eq!(messages(SOURCES[locale.index()]), keys);
            let i18n = I18n::new(LocalePreference::Explicit(locale), Locale::En);
            for key in &keys {
                let translated = i18n.format(key, &values);
                assert!(!translated.is_empty());
                assert!(!translated.contains(['\u{2068}', '\u{2069}']));
                // Removing each possible argument must fail in exactly the same
                // messages across locales; catches translated/omitted variable names.
                for (name, _) in values {
                    let reduced: Vec<_> = values.into_iter().filter(|(n, _)| *n != name).collect();
                    let mut args = FluentArgs::new();
                    for (n, v) in reduced {
                        args.set(n, v);
                    }
                    let succeeds = |language: Locale| {
                        let bundle = &i18n.catalogs[language.index()];
                        let message = bundle.get_message(key).unwrap();
                        let mut errors = vec![];
                        let _ = bundle.format_pattern(
                            message.value().unwrap(),
                            Some(&args),
                            &mut errors,
                        );
                        errors.is_empty()
                    };
                    assert_eq!(succeeds(locale), succeeds(Locale::En), "{key}/{name}");
                }
            }
            assert!(i18n.diagnostics().is_empty(), "{:?}", i18n.diagnostics());
        }
    }

    #[test]
    fn missing_or_invalid_translations_fallback_and_report_once() {
        let mut i18n = I18n::new(LocalePreference::Explicit(Locale::ZhCn), Locale::En);
        i18n.catalogs = Arc::new(catalogs([
            "hello = Hello { $name }\nmissing = Fallback\n",
            "hello = 错误 { $wrong }\n",
            "",
        ]));
        for _ in 0..3 {
            assert_eq!(i18n.format("hello", &[("name", "Maka")]), "Hello Maka");
            assert_eq!(i18n.text("missing"), "Fallback");
            assert_eq!(i18n.text("unknown"), "[unknown]");
        }
        assert_eq!(i18n.diagnostics().len(), 4);
    }
}

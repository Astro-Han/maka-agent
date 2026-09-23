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

use crate::i18n::I18n;
use maka_protocol::context::ContextDiagnosticsResult;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Request {
    pub session: String,
    pub generation: u64,
}

/// Latest completed model request, never a sum of transcript token usage.
#[derive(Default)]
pub struct Context {
    value: Option<ContextDiagnosticsResult>,
    wanted: bool,
    inflight: bool,
    last_request: Option<Instant>,
    ready_at: Option<Instant>,
}
impl Context {
    pub fn refresh(&mut self) {
        self.wanted = true;
        self.ready_at = None;
    }
    pub fn refresh_stream(&mut self) {
        if !self.wanted {
            self.wanted = true;
            self.ready_at = self.last_request.map(|at| at + Duration::from_millis(250));
        }
    }
    pub fn wait(&self) -> Option<Duration> {
        (self.wanted && !self.inflight).then(|| {
            self.ready_at.map_or(Duration::ZERO, |at| {
                at.saturating_duration_since(Instant::now())
            })
        })
    }
    pub fn request(&mut self, session: &str, generation: u64) -> Option<Request> {
        if !self.wanted || self.inflight || self.wait().is_some_and(|wait| !wait.is_zero()) {
            return None;
        }
        self.wanted = false;
        self.inflight = true;
        self.last_request = Some(Instant::now());
        Some(Request {
            session: session.into(),
            generation,
        })
    }
    pub fn complete(&mut self, value: Result<ContextDiagnosticsResult, String>) {
        self.inflight = false;
        // An unavailable/failed query must not masquerade as a current measurement.
        self.value = value.ok();
    }
    pub fn label(&self, i18n: &I18n) -> Option<String> {
        let ContextDiagnosticsResult::Available {
            input_tokens: Some(input),
            context_window: Some(window),
            ..
        } = self.value.as_ref()?
        else {
            return None;
        };
        Some(i18n.format(
            "chat-context-last",
            &[("input", &compact(*input)), ("window", &compact(*window))],
        ))
    }
    pub fn current_label(
        &self,
        model: &str,
        connection: Option<&str>,
        ascii: bool,
    ) -> Option<String> {
        let ContextDiagnosticsResult::Available {
            model_id,
            current: Some(usage),
            context_window: Some(window),
            ..
        } = self.value.as_ref()?
        else {
            return None;
        };
        if model != model_id || connection != Some(usage.connection_id.as_str()) {
            return None;
        }
        let mark = if usage.approximate {
            if ascii { "~" } else { "≈" }
        } else {
            ""
        };
        Some(format!(
            "{mark}{} / {}",
            compact(usage.tokens),
            compact(*window)
        ))
    }
}
fn compact(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1000 {
        format!("{:.1}k", value as f64 / 1000.0)
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Locale, LocalePreference};
    use maka_protocol::context::ContextDiagnosticsUnavailableReason;

    #[test]
    fn queries_coalesce_and_missing_measurements_never_become_zero_usage() {
        let mut context = Context::default();
        context.refresh();
        assert_eq!(context.request("a", 1).unwrap().session, "a");
        for _ in 0..10 {
            context.refresh();
            assert!(context.request("a", 1).is_none());
        }
        context.complete(Ok(ContextDiagnosticsResult::Available {
            provider_id: "p".into(),
            model_id: "model".into(),
            completed_at: 1,
            input_tokens: Some(9500),
            current: Some(maka_protocol::context::ContextUsage {
                connection_id: "connection".into(),
                tokens: 10500,
                approximate: true,
            }),
            cache_read_input_tokens: Some(8000),
            context_window: Some(128000),
            composition: None,
            compaction: None,
        }));
        for locale in Locale::ALL {
            let label = context
                .label(&I18n::new(LocalePreference::Explicit(locale), locale))
                .unwrap();
            assert!(label.contains("9.5k") && label.contains("128.0k"));
            assert!(
                !label.contains("17.5k"),
                "cached tokens are already included in total input"
            );
        }
        assert_eq!(
            context
                .current_label("model", Some("connection"), false)
                .as_deref(),
            Some("≈10.5k / 128.0k")
        );
        assert_eq!(
            context
                .current_label("model", Some("connection"), true)
                .as_deref(),
            Some("~10.5k / 128.0k")
        );
        assert!(
            context
                .current_label("changed", Some("connection"), false)
                .is_none()
        );
        assert!(
            context
                .current_label("model", Some("other"), false)
                .is_none()
        );
        assert!(context.request("a", 1).is_some());
        context.complete(Ok(ContextDiagnosticsResult::Unavailable {
            reason: ContextDiagnosticsUnavailableReason::NoCompletedRequest,
        }));
        assert!(context.request("a", 1).is_none());
        context.refresh_stream();
        assert!(context.wait().is_some_and(|wait| !wait.is_zero()));
        assert!(context.request("a", 1).is_none());
        context.refresh(); // A settled usage event need not wait for stream throttling.
        assert!(context.request("a", 1).is_some());
        context.complete(Err("offline".into()));
        assert!(
            context
                .label(&I18n::new(
                    LocalePreference::Explicit(Locale::En),
                    Locale::En
                ))
                .is_none()
        );
        let mut chat = super::super::Chat::default();
        chat.select(&crate::navigation::Route::Session("a".into()));
        chat.context.refresh();
        let stale = chat.context.request("a", chat.generation).unwrap();
        chat.select(&crate::navigation::Route::Session("b".into()));
        chat.context.refresh();
        let current = chat.context.request("b", chat.generation).unwrap();
        chat.context_completed(stale, Err("old connection".into()));
        chat.context.refresh();
        assert!(
            chat.context.request("b", chat.generation).is_none(),
            "stale completion cannot clear the new in-flight query"
        );
        chat.context_completed(current, Err("offline".into()));
        assert!(chat.context.request("b", chat.generation).is_some());
    }
}

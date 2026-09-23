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

//! Interpret only the native foreground Shell envelope, never arbitrary output text.
use super::State;
use serde_json::Value;

pub(super) fn summary(
    call: Option<&Value>,
    content: &Value,
    i18n: &crate::i18n::I18n,
) -> Option<String> {
    shell(call, content)?;
    let value = &content["value"];
    if let Some(code) = value["exitCode"].as_i64() {
        Some(i18n.format("tool-exit-code", &[("code", &code.to_string())]))
    } else {
        value["failureMessage"].as_str().map(str::to_owned)
    }
}

pub(super) fn shell(call: Option<&Value>, content: &Value) -> Option<State> {
    let call = call?;
    if call["toolName"] != "Shell"
        || !matches!(call["origin"].as_str(), Some("provider" | "code_mode"))
        || content["kind"] != "json"
    {
        return None;
    }
    let value = &content["value"];
    if value["kind"] != "terminal"
        || !value["cwd"].is_string()
        || !value["cmd"].is_string()
        || value["cmd"] != call["args"]["command"]
        || value["output"]["mode"] != "pipes"
        || !["stdout", "stderr"]
            .iter()
            .all(|key| value["output"][key].is_string())
        || !["stdoutTruncated", "stderrTruncated", "redacted"]
            .iter()
            .all(|key| value["output"][key].is_boolean())
    {
        return None;
    }
    let code = value["exitCode"].as_i64();
    match value["status"].as_str()? {
        "completed" if code == Some(0) && value.get("failureMessage").is_none() => {
            Some(State::Completed)
        }
        "failed"
            if code.is_some_and(|code| code != 0)
                || value.get("exitCode").is_none()
                    && value["failureMessage"]
                        .as_str()
                        .is_some_and(|text| !text.is_empty()) =>
        {
            Some(State::Failed)
        }
        "timed_out" if code == Some(124) => Some(State::TimedOut),
        "cancelled" if code == Some(130) => Some(State::Cancelled),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::Card;
    use super::*;
    use crate::i18n::{I18n, Locale, LocalePreference};
    use serde_json::json;
    #[test]
    fn tool_outcomes_require_canonical_evidence_and_remain_visible_without_color() {
        let call = json!({"toolName":"Shell","origin":"code_mode","args":{"command":"exit 7"}});
        for (status, code, state) in [
            ("failed", 7, State::Failed),
            ("completed", 0, State::Completed),
            ("timed_out", 124, State::TimedOut),
            ("cancelled", 130, State::Cancelled),
        ] {
            let result = json!({"isError":false,"content":{"kind":"json","value":{"kind":"terminal","cwd":"/tmp","cmd":"exit 7","status":status,"exitCode":code,
                "output":{"mode":"pipes","stdout":"error is a word, not evidence","stderr":"","stdoutTruncated":false,"stderrTruncated":false,"redacted":false}}}});
            let card = Card {
                turn: "t",
                id: "s",
                call: Some((1, &call)),
                result: Some((2, &result)),
                closed: true,
            };
            assert!(card.state(false) == state);
            for locale in Locale::ALL {
                let i18n = I18n::new(LocalePreference::Explicit(locale), locale);
                let text = card.text(state, &i18n, false).text;
                assert!(text.contains(&i18n.text(state.label())));
                if state.problem() {
                    assert!(
                        text.lines()
                            .next()
                            .unwrap()
                            .contains(&i18n.text(state.label()))
                    );
                }
            }
            let mut foreign = call.clone();
            foreign["toolName"] = json!("mcp__remote__Shell");
            assert!(shell(Some(&foreign), &result["content"]).is_none());
            let mut malformed = result["content"].clone();
            malformed["value"]["cmd"] = json!("another command");
            assert!(shell(Some(&call), &malformed).is_none());
        }
        assert!(
            shell(
                Some(&call),
                &json!({"kind":"text","text":"ERROR exitCode: 9"})
            )
            .is_none()
        );
        assert!(
            shell(
                Some(&call),
                &json!({"kind":"json","value":{"status":"failed","exitCode":9}})
            )
            .is_none()
        );
        let result = json!({"isError":true,"content":{"kind":"text","text":"rejected"}});
        let card = Card {
            turn: "t",
            id: "s",
            call: Some((1, &call)),
            result: Some((2, &result)),
            closed: true,
        };
        assert!(card.state(false) == State::Attention);
    }
}

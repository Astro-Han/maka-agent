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

use super::ToolMode;
use serde::{Deserialize, Serialize};

impl ToolMode {
    /// Product default, not capability detection. A model override always wins.
    pub fn for_model(model_id: &str, request_url: &str, enabled: Option<bool>) -> Self {
        let enabled = enabled.unwrap_or_else(|| {
            let model = model_id.to_ascii_lowercase();
            model.contains("gpt-") || model.contains("deepseek") || deepseek_endpoint(request_url)
        });
        if enabled {
            Self::CodeMode
        } else {
            Self::Direct
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditingTools {
    #[default]
    Structured,
    ApplyPatch,
}

impl EditingTools {
    /// Deliberately independent of Code Mode: coding and patch proficiency differ.
    pub fn for_model(model_id: &str, request_url: &str, enabled: Option<bool>) -> Self {
        let enabled = enabled.unwrap_or_else(|| {
            let model = model_id.to_ascii_lowercase();
            model.contains("gpt-") || model.contains("deepseek") || deepseek_endpoint(request_url)
        });
        if enabled {
            Self::ApplyPatch
        } else {
            Self::Structured
        }
    }
}

fn deepseek_endpoint(request_url: &str) -> bool {
    url::Url::parse(request_url).ok().is_some_and(|url| {
        matches!(url.scheme(), "https" | "http") && url.host_str() == Some("api.deepseek.com")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_work_through_gateways_and_overrides_remain_independent() {
        for (model, url, enabled) in [
            ("openai/gpt-next", "https://openrouter.ai/api/v1", true),
            ("GPT-5.6-luna", "http://localhost:8888/v1", true),
            (
                "deepseek/deepseek-v4.1-flash",
                "http://localhost:8888/v1",
                true,
            ),
            ("deployment", "https://api.deepseek.com/v1", true),
            ("other", "https://api.openai.com/v1", false),
            ("other", "https://api.deepseek.com.example/v1", false),
            ("other", "https://example.com/api.deepseek.com", false),
            ("other", "not a URL", false),
        ] {
            assert_eq!(
                ToolMode::for_model(model, url, None) == ToolMode::CodeMode,
                enabled
            );
            assert_eq!(
                EditingTools::for_model(model, url, None) == EditingTools::ApplyPatch,
                enabled
            );
            for code in [false, true] {
                for patch in [false, true] {
                    assert_eq!(
                        ToolMode::for_model(model, url, Some(code)) == ToolMode::CodeMode,
                        code
                    );
                    assert_eq!(
                        EditingTools::for_model(model, url, Some(patch))
                            == EditingTools::ApplyPatch,
                        patch
                    );
                }
            }
        }
    }
}

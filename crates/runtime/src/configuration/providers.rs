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
//! Provider endpoint/auth snapshot from core/provider-registry.ts (2026-09-11).
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAuthKind {
    ApiKey,
    OauthToken,
    None,
    OptionalApiKey,
}

pub fn provider_default_base_url(provider: &str) -> Result<&'static str, String> {
    match provider {
        "anthropic" => Ok("https://api.anthropic.com"),
        "kimi-coding-plan" => Ok("https://api.kimi.com/coding/v1"),
        "minimax-coding-plan" => Ok("https://api.minimax.io/anthropic"),
        "tencent-coding-plan" => Ok("https://api.lkeap.cloud.tencent.com/coding/v3"),
        "volcengine-coding-plan" => Ok("https://ark.cn-beijing.volces.com/api/coding/v3"),
        "volcengine-agent-plan" => Ok("https://ark.cn-beijing.volces.com/api/plan/v3"),
        "tencent-token-plan" => Ok("https://api.lkeap.cloud.tencent.com/plan/v3"),
        "openai" => Ok("https://api.openai.com/v1"),
        "google" => Ok("https://generativelanguage.googleapis.com/v1beta"),
        "deepseek" => Ok("https://api.deepseek.com"),
        "moonshot" => Ok("https://api.moonshot.cn/v1"),
        "zai-coding-plan" => Ok("https://api.z.ai/api/coding/paas/v4"),
        "MiniMax" => Ok("https://api.minimax.io/anthropic/v1"),
        "MiniMax-cn" => Ok("https://api.minimaxi.com/anthropic/v1"),
        "siliconflow" => Ok("https://api.siliconflow.com/v1"),
        "vercel" => Ok("https://ai-gateway.vercel.sh/v1"),
        "xai" => Ok("https://api.x.ai/v1"),
        "xai-oauth" => Ok("https://api.x.ai/v1"),
        "zai" => Ok("https://api.z.ai/api/paas/v4"),
        "xiaomi" => Ok("https://api.xiaomimimo.com/v1"),
        "xiaomi-token-plan-cn" => Ok("https://token-plan-cn.xiaomimimo.com/v1"),
        "xiaomi-token-plan-sgp" => Ok("https://token-plan-sgp.xiaomimimo.com/v1"),
        "xiaomi-token-plan-ams" => Ok("https://token-plan-ams.xiaomimimo.com/v1"),
        "cerebras" => Ok("https://api.cerebras.ai/v1"),
        "mistral" => Ok("https://api.mistral.ai/v1"),
        "cohere" => Ok("https://api.cohere.com/v2"),
        "huggingface" => Ok("https://router.huggingface.co/v1"),
        "zenmux" => Ok("https://zenmux.ai/api/v1"),
        "opencode" => Ok("https://opencode.ai/zen/v1"),
        "opencode-go" => Ok("https://opencode.ai/zen/go/v1"),
        "opencode-free" => Ok("https://opencode.ai/zen/v1"),
        "togetherai" => Ok("https://api.together.ai/v1"),
        "fireworks-ai" => Ok("https://api.fireworks.ai/inference/v1/"),
        "nvidia" => Ok("https://integrate.api.nvidia.com/v1"),
        "tencent-tokenhub" => Ok("https://tokenhub.tencentmaas.com/v1"),
        "stepfun" => Ok("https://api.stepfun.com/v1"),
        "stepfun-step-plan" => Ok("https://api.stepfun.com/step_plan/v1"),
        "stepfun-ai-step-plan" => Ok("https://api.stepfun.ai/step_plan/v1"),
        "stepfun-ai" => Ok("https://api.stepfun.ai/v1"),
        "volcengine-ark" => Ok("https://ark.cn-beijing.volces.com/api/v3"),
        "deepinfra" => Ok("https://api.deepinfra.com/v1/openai"),
        "groq" => Ok("https://api.groq.com/openai/v1"),
        "openrouter" => Ok("https://openrouter.ai/api/v1"),
        "alibaba" => Ok("https://dashscope-intl.aliyuncs.com/compatible-mode/v1"),
        "alibaba-cn" => Ok("https://dashscope.aliyuncs.com/compatible-mode/v1"),
        "alibaba-coding-plan-cn" => Ok("https://coding.dashscope.aliyuncs.com/v1"),
        "alibaba-coding-plan" => Ok("https://coding-intl.dashscope.aliyuncs.com/v1"),
        "alibaba-token-plan-cn" => {
            Ok("https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1")
        }
        "alibaba-token-plan" => {
            Ok("https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1")
        }
        "commandcode" => Ok("https://api.commandcode.ai/provider/v1"),
        "cloudflare-workers-ai" => Ok(""),
        "ollama-cloud" => Ok("https://ollama.com/v1"),
        "ollama" => Ok("http://127.0.0.1:11434/v1"),
        "lm-studio" => Ok("http://127.0.0.1:1234/v1"),
        "localai" => Ok("http://127.0.0.1:8080/v1"),
        "openai-compatible" => Ok(""),
        "openai-responses-compatible" => Ok(""),
        "anthropic-compatible" => Ok(""),
        "github-copilot" => Ok("https://api.githubcopilot.com"),
        "claude-subscription" => Ok("https://api.anthropic.com"),
        "openai-codex" => Ok("https://chatgpt.com/backend-api/codex"),
        _ => Err("connection provider type is not registered".into()),
    }
}
pub fn provider_auth_kind(provider: &str) -> Result<ProviderAuthKind, String> {
    provider_default_base_url(provider)?;
    Ok(match provider {
        "xai-oauth" | "github-copilot" | "claude-subscription" | "openai-codex" => {
            ProviderAuthKind::OauthToken
        }
        "ollama" | "lm-studio" | "opencode-free" => ProviderAuthKind::None,
        "localai" => ProviderAuthKind::OptionalApiKey,
        _ => ProviderAuthKind::ApiKey,
    })
}

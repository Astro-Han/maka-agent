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

use crate::{Error, MAX_DELAY_MS, invalid, schedule::Schedule};
use maka_runtime::execution::{BehaviorId, CollaborationMode, SandboxMode, ThinkingLevel};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Active,
    Paused,
    Completed,
    Expired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok,
    Failed,
    Blocked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bot {
    Telegram,
    Feishu,
    Wecom,
    Wechat,
    Discord,
    Dingtalk,
    Qq,
    Slack,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "channel",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Notification {
    Local,
    Bot { platform: Bot, chat_id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Effect {
    Notify(Notification),
    SessionResume { session_id: String },
    AgentRun { execution: ExecutionTemplate },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionTemplate {
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub llm_connection_id: String,
    pub llm_connection_slug: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<ThinkingLevel>,
    pub sandbox_mode: SandboxMode,
    pub approval_policy: maka_runtime::execution::ApprovalPolicy,
    pub collaboration_mode: CollaborationMode,
    pub orchestration_mode: BehaviorId,
}
impl Effect {
    pub fn validate(&self) -> Result<(), Error> {
        match self {
            Self::Notify(Notification::Local) => Ok(()),
            Self::Notify(Notification::Bot { chat_id, .. }) => text(chat_id, 160),
            Self::SessionResume { session_id } => text(session_id, 160),
            Self::AgentRun { execution } => {
                text(&execution.cwd, 4096)?;
                for value in [
                    &execution.llm_connection_id,
                    &execution.llm_connection_slug,
                    &execution.model,
                ] {
                    text(value, 256)?;
                }
                if let Some(project) = &execution.project_id {
                    text(project, 256)?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Creator {
    User,
    Agent { session_id: String },
    System,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Intent {
    Text { body: String },
}
impl Intent {
    pub fn body(&self) -> &str {
        match self {
            Self::Text { body } => body,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Run {
    pub id: String,
    pub at: i64,
    pub outcome: Outcome,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub intent: Intent,
    pub schedule: Schedule,
    pub effect: Effect,
    pub status: Status,
    pub next_fire_at: Option<i64>,
    pub last_fire_at: Option<i64>,
    pub fire_count: u32,
    pub max_fires: Option<u32>,
    pub expires_at: Option<i64>,
    pub created_by: Creator,
    pub created_at: i64,
    pub updated_at: i64,
    pub runs: Vec<Run>,
    pub last_error: Option<String>,
}
impl Task {
    pub(crate) fn bound_history(&mut self) -> Result<(), Error> {
        while serde_json::to_vec(self)
            .map_err(|error| invalid(error.to_string()))?
            .len()
            > 87 * 1024
        {
            if self.runs.pop().is_none() {
                return Err(invalid("task exceeds response budget"));
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Create {
    pub title: String,
    pub intent_body: String,
    pub schedule: Schedule,
    pub effect: Effect,
    #[serde(default)]
    pub max_fires: Option<u32>,
    #[serde(default)]
    pub expires_at: Option<i64>,
}
impl Create {
    pub fn validate(&self, now: i64) -> Result<(), Error> {
        self.validate_fields()?;
        if self.expires_at.is_some_and(|expires| expires <= now) {
            return Err(invalid("expiry must be a future timestamp"));
        }
        if let Schedule::Once { run_at } = self.schedule
            && (run_at <= now || run_at.saturating_sub(now) > MAX_DELAY_MS)
        {
            return Err(invalid(
                "one-shot trigger must be within the following year",
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_fields(&self) -> Result<(), Error> {
        // Leave room for metadata, lastError and at least one complete run.
        if serde_json::to_vec(self)
            .map_err(|error| invalid(error.to_string()))?
            .len()
            > 64 * 1024
        {
            return Err(invalid("task input exceeds 64 KiB encoded"));
        }
        text(&self.title, 120)?;
        if !matches!(self.effect, Effect::Notify(_)) {
            text(&self.intent_body, 8000)?;
        } else if self.intent_body.chars().count() > 8000 {
            return Err(invalid("intent exceeds 8000 characters"));
        }
        self.schedule.validate()?;
        self.effect.validate()?;
        if self
            .max_fires
            .is_some_and(|max| !(1..=10_000).contains(&max))
        {
            return Err(invalid("maxFires must be between 1 and 10000"));
        }
        if self
            .expires_at
            .is_some_and(|expires| !(0..=(1_i64 << 53) - 1).contains(&expires))
        {
            return Err(invalid("expiry must be a valid timestamp"));
        }
        Ok(())
    }
}
pub(crate) fn text(value: &str, maximum: usize) -> Result<(), Error> {
    if value.trim().is_empty() || value.chars().count() > maximum || value.contains('\0') {
        return Err(invalid("empty or oversized task field"));
    }
    Ok(())
}

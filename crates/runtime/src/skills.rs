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

type Result<T> = std::result::Result<T, &'static str>;
use serde::{Deserialize, Serialize};
mod receipt;
mod validation;
pub use receipt::{SkillFailedReceipt, SkillInvocationReceipt, SkillLoadedReceipt};
pub use validation::SkillValidationCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillPreference {
    pub enabled: bool,
    pub pinned: bool,
}
impl Default for SkillPreference {
    fn default() -> Self {
        Self {
            enabled: true,
            pinned: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillInvocationResult {
    pub loaded: Vec<LoadedSkill>,
    pub failed: Vec<SkillInvocationFailure>,
    pub receipts: Vec<SkillInvocationReceipt>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadedSkill {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SkillInvocationFailure {
    Request(SkillRequestFailure),
    Overflow(SkillOverflowFailure),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillRequestFailure {
    pub request: String,
    pub reason: SkillFailureReason,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillOverflowFailure {
    pub reason: TooManyRequests,
    pub request_limit: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TooManyRequests {
    TooManyRequests,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillFailureReason {
    InvalidName,
    NotFound,
    Disabled,
    HostIncompatible,
    ResolutionFailed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillInvocationMode {
    Explicit,
    ModelTool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillScope {
    Project,
    Workspace,
    User,
    Custom,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    Maka,
    Agents,
    Legacy,
    Custom,
}

impl SkillInvocationResult {
    pub fn is_empty(&self) -> bool {
        self.loaded.is_empty() && self.failed.is_empty() && self.receipts.is_empty()
    }

    pub fn validate(&self) -> Result<()> {
        ensure(
            self.loaded.len() <= 50 && self.failed.len() <= 50 && self.receipts.len() <= 50,
            "Too many skill invocation entries",
        )?;
        for loaded in &self.loaded {
            identity(&loaded.id, &loaded.name)?;
        }
        for failed in &self.failed {
            match failed {
                SkillInvocationFailure::Request(failure) => bytes(&failure.request, 512, false)?,
                SkillInvocationFailure::Overflow(failure) => limit(failure.request_limit)?,
            }
        }
        for receipt in &self.receipts {
            match receipt {
                SkillInvocationReceipt::Loaded(r) => {
                    bytes(&r.request, 512, false)?;
                    bytes(&r.skill_ref, 512, false)?;
                    identity(&r.id, &r.name)?;
                }
                SkillInvocationReceipt::Failed(r) => {
                    bytes(&r.request, 512, false)?;
                }
                SkillInvocationReceipt::Overflow { request_limit } => {
                    limit(*request_limit)?;
                }
            }
        }
        encoded(self, 72 * 1024)
    }
}
fn identity(id: &str, name: &str) -> Result<()> {
    bytes(id, 128, false)?;
    bytes(name, 256, false)
}
fn limit(limit: u64) -> Result<()> {
    ensure((1..=50).contains(&limit), "Invalid skill request limit")
}

fn ensure(valid: bool, message: &'static str) -> Result<()> {
    if valid { Ok(()) } else { Err(message) }
}
fn bytes(value: &str, max: usize, empty: bool) -> Result<()> {
    ensure(
        (empty || !value.is_empty()) && value.len() <= max,
        "Invalid UTF-8 string",
    )
}
fn encoded(value: &impl Serialize, max: usize) -> Result<()> {
    ensure(
        serde_json::to_vec(value)
            .map_err(|_| "Invalid skill result")?
            .len()
            <= max,
        "Encoded skill result exceeds byte limit",
    )
}

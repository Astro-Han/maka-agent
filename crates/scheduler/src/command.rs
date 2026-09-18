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

use crate::{
    schedule::Schedule,
    task::{Create, Effect, Task},
};
use maka_runtime::configuration::Patch;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Query {
    List {
        cursor: Option<String>,
        expected_revision: Option<u64>,
    },
    Get {
        task_id: String,
    },
}
impl Query {
    pub fn validate(&self) -> Result<(), crate::Error> {
        match self {
            Self::Get { task_id } => crate::task::text(task_id, 160),
            Self::List {
                cursor,
                expected_revision,
            } => {
                if cursor.is_some() != expected_revision.is_some()
                    || cursor.as_ref().is_some_and(|cursor| {
                        cursor.is_empty()
                            || cursor.len() > 160
                            || !cursor.bytes().all(|byte| byte.is_ascii_digit())
                    })
                    || expected_revision.is_some_and(|revision| revision > (1 << 53) - 1)
                {
                    return Err(crate::invalid(
                        "continuation requires a numeric cursor and revision",
                    ));
                }
                Ok(())
            }
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Mutation {
    Create { input: Create },
    Update { task_id: String, patch: Update },
    Pause { task_id: String },
    Resume { task_id: String },
    ClearHistory { task_id: String },
    TriggerNow { task_id: String },
    Delete { task_id: String },
    Snooze { task_id: String, delay_ms: i64 },
}
impl Mutation {
    pub fn uses_host_paths(&self) -> bool {
        let effect = match self {
            Self::Create { input } => Some(&input.effect),
            Self::Update { patch, .. } => patch.effect.as_ref(),
            _ => None,
        };
        matches!(effect, Some(Effect::AgentRun { execution }) if execution.project_id.is_none())
    }
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Update {
    #[serde(default, deserialize_with = "present")]
    pub title: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub intent_body: Option<String>,
    #[serde(default, deserialize_with = "present")]
    pub schedule: Option<Schedule>,
    #[serde(default, deserialize_with = "present")]
    pub effect: Option<Effect>,
    #[serde(default)]
    pub max_fires: Patch<u32>,
    #[serde(default)]
    pub expires_at: Patch<i64>,
}
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum QueryResult {
    Page {
        revision: u64,
        tasks: Vec<Task>,
        next_cursor: Option<String>,
    },
    RevisionChanged {
        expected: u64,
        actual: u64,
    },
    Task {
        task: Option<Box<Task>>,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum MutationResult {
    Task { task: Box<Task> },
    Deleted { task_id: String },
}

fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

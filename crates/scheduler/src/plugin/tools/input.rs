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
    task::{Create, Effect, Notification},
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "mode",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum Input {
    Create {
        #[schemars(length(min = 1, max = 120))]
        title: String,
        #[schemars(length(min = 1, max = 8000))]
        intent_body: String,
        schedule: When,
        #[serde(default)]
        effect: Target,
        #[schemars(range(min = 1, max = 10000))]
        max_fires: Option<u32>,
    },
    List {},
    Pause {
        #[schemars(length(min = 1, max = 128))]
        id: String,
    },
    Resume {
        #[schemars(length(min = 1, max = 128))]
        id: String,
    },
    Delete {
        #[schemars(length(min = 1, max = 128))]
        id: String,
    },
}
#[derive(Default, Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum Target {
    #[default]
    SessionResume,
    AgentRun,
    NotifyLocal,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum When {
    Once {
        #[schemars(range(min = 1, max = 9007199254740991_i64))]
        run_at: i64,
    },
    Interval {
        #[schemars(range(min = 10, max = 31622400))]
        every_seconds: u32,
        #[schemars(range(min = 1, max = 9007199254740991_i64))]
        start_at: Option<i64>,
    },
    Cron {
        #[schemars(length(min = 9, max = 160))]
        expression: String,
        #[schemars(range(min = 1, max = 9007199254740991_i64))]
        start_at: Option<i64>,
    },
}
impl When {
    pub(super) fn resolve(self) -> Schedule {
        let now = jiff::Timestamp::now().as_millisecond();
        match self {
            Self::Once { run_at } => Schedule::Once { run_at },
            Self::Interval {
                every_seconds,
                start_at,
            } => Schedule::Interval {
                every_seconds,
                start_at: start_at.unwrap_or(now),
            },
            Self::Cron {
                expression,
                start_at,
            } => Schedule::Cron {
                expression,
                start_at: start_at.unwrap_or(now),
            },
        }
    }
}
impl Input {
    pub(super) async fn mutation(
        self,
        operations: &super::super::backend::Backend,
        invocation: &maka_runtime::event::Invocation,
    ) -> Result<crate::command::Mutation, crate::Error> {
        use crate::command::Mutation;
        Ok(match self {
            Self::Create {
                title,
                intent_body,
                schedule,
                effect,
                max_fires,
            } => {
                let effect = match effect {
                    Target::SessionResume => Effect::SessionResume {
                        session_id: invocation.session_id.clone(),
                    },
                    Target::NotifyLocal => Effect::Notify(Notification::Local),
                    Target::AgentRun => Effect::AgentRun {
                        execution: operations
                            .template(maka_plugins::call::current().ok_or_else(|| {
                                crate::Error::Invalid("missing Agent authority".into())
                            })?)
                            .await?,
                    },
                };
                Mutation::Create {
                    input: Create {
                        title,
                        intent_body,
                        schedule: schedule.resolve(),
                        effect,
                        max_fires,
                        expires_at: None,
                    },
                }
            }
            Self::Pause { id } => Mutation::Pause { task_id: id },
            Self::Resume { id } => Mutation::Resume { task_id: id },
            Self::Delete { id } => Mutation::Delete { task_id: id },
            Self::List {} => {
                return Err(crate::Error::Invalid("list is not a mutation".into()));
            }
        })
    }
}
pub(super) fn schema() -> Value {
    schemars::schema_for!(Input).into()
}

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
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum Input {
    Create {
        title: String,
        intent_body: String,
        schedule: When,
        #[serde(default)]
        effect: Target,
        max_fires: Option<u32>,
    },
    List {},
    Pause {
        id: String,
    },
    Resume {
        id: String,
    },
    Delete {
        id: String,
    },
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Target {
    #[default]
    SessionResume,
    AgentRun,
    NotifyLocal,
}
#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(super) enum When {
    Once {
        run_at: i64,
    },
    Interval {
        every_seconds: u32,
        start_at: Option<i64>,
    },
    Cron {
        expression: String,
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
    let time = json!({"type":"integer","minimum":1,"maximum":9007199254740991_u64});
    let once = json!({"type":"object","properties":{"kind":{"const":"once"},"runAt":time},"required":["kind","runAt"],"additionalProperties":false});
    let interval = json!({"type":"object","properties":{"kind":{"const":"interval"},"everySeconds":{"type":"integer","minimum":10,"maximum":31622400},"startAt":time},"required":["kind","everySeconds"],"additionalProperties":false});
    let cron = json!({"type":"object","properties":{"kind":{"const":"cron"},"expression":{"type":"string","minLength":9,"maxLength":160},"startAt":time},"required":["kind","expression"],"additionalProperties":false});
    json!({"oneOf":[
        {"type":"object","properties":{
            "mode":{"const":"create"},"title":{"type":"string","minLength":1,"maxLength":120},
            "intentBody":{"type":"string","minLength":1,"maxLength":8000},
            "schedule":{"oneOf":[once,interval,cron]},
            "effect":{"enum":["session_resume","agent_run","notify_local"]},
            "maxFires":{"type":"integer","minimum":1,"maximum":10000}
        },"required":["mode","title","intentBody","schedule"],"additionalProperties":false},
        {"type":"object","properties":{"mode":{"const":"list"}},"required":["mode"],"additionalProperties":false},
        {"type":"object","properties":{"mode":{"enum":["pause","resume","delete"]},"id":{"type":"string","minLength":1,"maxLength":128}},"required":["mode","id"],"additionalProperties":false}
    ]})
}

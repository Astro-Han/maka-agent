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

//! Physical model attempts; independent of conversation membership and current prices.

use crate::{context::ModelPurpose, event::Invocation, execution::ModelBinding, model::ModelUsage};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Success,
    Error,
    Aborted,
    /// The admission ended without a recorded provider outcome.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuxiliarySource {
    Agent {
        invocation: Invocation,
        operation_id: String,
    },
    HostEffect {
        id: Uuid,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    Agent {
        invocation: Invocation,
        purpose: ModelPurpose,
    },
    Auxiliary {
        source: AuxiliarySource,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelAttempt {
    pub request_id: String,
    pub origin: Origin,
    pub session_id: Option<String>,
    pub binding: Option<ModelBinding>,
    pub model_id: String,
    pub started_at: f64,
    pub completed_at: f64,
    pub outcome: Outcome,
    pub usage: ModelUsage,
    pub quote: Option<crate::pricing::Quote>,
    pub cost_usd: Option<f64>,
}

impl ModelAttempt {
    /// Wall-clock adjustments cannot produce negative reported latency.
    pub fn latency_ms(&self) -> f64 {
        (self.completed_at - self.started_at).max(0.0)
    }
}

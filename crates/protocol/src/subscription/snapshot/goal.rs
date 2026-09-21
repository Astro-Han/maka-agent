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

use super::super::{ensure, entity};
use super::{count, encoded};
use crate::Result;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Waiting,
    Achieved,
    Impossible,
    Cleared,
    Paused,
    Stalled,
    BudgetLimited,
    MaxIterations,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalProjection {
    pub goal_id: String,
    pub revision: u64,
    pub session_id: String,
    pub condition: String,
    pub status: GoalStatus,
    pub set_at: u64,
    pub iterations: u64,
    pub max_iterations: u64,
    pub consecutive_no_progress: u64,
    pub block_cap: u64,
    pub token_budget: Option<u64>,
    pub tokens_spent: u64,
    pub last_reason: Option<String>,
    pub achieved_at: Option<u64>,
    pub paused_at: Option<u64>,
}

impl GoalProjection {
    pub fn validate(&self) -> Result<()> {
        entity(&self.goal_id)?;
        entity(&self.session_id)?;
        for value in [
            self.revision,
            self.set_at,
            self.iterations,
            self.consecutive_no_progress,
            self.tokens_spent,
        ] {
            count(value, false)?;
        }
        for value in [self.max_iterations, self.block_cap] {
            count(value, true)?;
        }
        if let Some(value) = self.token_budget {
            count(value, true)?;
        }
        for value in [self.achieved_at, self.paused_at].into_iter().flatten() {
            count(value, false)?;
        }
        for text in std::iter::once(&self.condition).chain(self.last_reason.iter()) {
            ensure(
                text.len() <= 1500 && text.encode_utf16().count() <= 500,
                "Goal text exceeds limit",
            )?;
        }
        encoded(self, 56 * 1024)
    }
}

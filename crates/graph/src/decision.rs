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
    Error, WorkId,
    schedule::{Finish, HistoricalInput, Source, Stop, Target, Update, Work},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Decision {
    AddWork {
        #[schemars(length(min = 1, max = 32))]
        work: Vec<NewWork>,
    },
    Stop {
        #[schemars(length(min = 1, max = 20))]
        targets: Vec<Stop>,
    },
    Finish {
        #[schemars(length(min = 1, max = 64), inner(length(min = 1, max = 256)))]
        result_ids: Vec<String>,
        #[schemars(length(min = 1, max = 4000))]
        reason: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NewWork {
    pub target: Target,
    #[schemars(length(min = 1, max = 60000))]
    pub instruction: String,
    #[serde(default)]
    #[schemars(length(max = 64), inner(length(min = 1, max = 256)))]
    pub input_ids: Vec<String>,
    #[serde(default)]
    #[schemars(length(max = 64))]
    pub selected_result_inputs: Vec<HistoricalInput>,
    #[schemars(length(max = 256))]
    pub replaces: Option<String>,
}
impl Decision {
    pub fn update(self, graph_id: crate::GraphId, source: Source) -> Result<Update, Error> {
        let mut update = Update {
            graph_id,
            source,
            add_work: vec![],
            stop: vec![],
            finish: None,
        };
        match self {
            Self::AddWork { work } => {
                for (index, work) in work.into_iter().enumerate() {
                    let data = serde_json::to_vec(&(&update.source, index))
                        .expect("typed source serializes");
                    let work_id: WorkId =
                        format!("graph_work_{:x}", Sha256::digest(data)).try_into()?;
                    update.add_work.push(Work {
                        work_id,
                        target: work.target,
                        instruction: work.instruction,
                        input_ids: work.input_ids,
                        selected_result_inputs: work.selected_result_inputs,
                        replaces: work.replaces,
                    });
                }
            }
            Self::Stop { targets } => update.stop = targets,
            Self::Finish { result_ids, reason } => {
                update.finish = Some(Finish { result_ids, reason })
            }
        }
        update.validate()?;
        Ok(update)
    }
}

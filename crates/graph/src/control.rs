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

use crate::{Epoch, Error, GraphId, OperatorId, WorkId};
use maka_plugins::execution::Submit;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Intent {
    pub graph_id: GraphId,
    pub work_id: WorkId,
    pub operator_id: OperatorId,
    pub request: Submit,
    pub schedule_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Wake {
    pub graph_id: GraphId,
    pub snapshot_key: String,
    pub request: Submit,
}

impl Wake {
    pub fn validate(&self) -> Result<(), Error> {
        crate::identity(&self.snapshot_key)?;
        self.request
            .validate()
            .map_err(|error| Error::Invalid(error.to_string()))
    }
}

impl Intent {
    pub fn validate(&self) -> Result<(), Error> {
        self.request
            .validate()
            .map_err(|error| Error::Invalid(error.to_string()))?;
        if self.schedule_revision == 0 || self.schedule_revision > (1 << 53) - 1 {
            return Err(Error::Invalid("invalid intent schedule revision".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct EpochPage {
    pub epochs: Vec<Epoch>,
    pub current_epoch: Option<u64>,
    pub next_before: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Control {
    pub epoch: Epoch,
    pub schedule_revision: u64,
    pub stop_requested: bool,
    pub finished: bool,
}

impl Control {
    pub fn closed(&self) -> bool {
        self.stop_requested || self.finished
    }
}

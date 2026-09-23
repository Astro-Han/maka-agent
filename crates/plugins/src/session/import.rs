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

use crate::execution::CreateRoot;
pub use maka_runtime::import::{Content, ImportProgress, ImportState, Record, Source};
use serde::{Deserialize, Serialize};

/// One root-creation capability, one durable import identity. An import cannot
/// write into an existing Session or supply canonical execution facts.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Command {
    Begin {
        root: Box<CreateRoot>,
        source: Source,
    },
    Append {
        operation_id: String,
        position: u64,
        records: Vec<Record>,
    },
    Publish {
        operation_id: String,
        records: u64,
    },
    Inspect {
        operation_id: String,
    },
    Abandon {
        operation_id: String,
    },
}
impl Command {
    pub fn operation_id(&self) -> &str {
        match self {
            Self::Begin { root, .. } => &root.operation_id,
            Self::Append { operation_id, .. }
            | Self::Publish { operation_id, .. }
            | Self::Inspect { operation_id }
            | Self::Abandon { operation_id } => operation_id,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub session_id: String,
    #[serde(flatten)]
    pub progress: ImportProgress,
}

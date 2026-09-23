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

//! Session history ownership is independent of execution identity.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CopyPurpose {
    Branch {
        /// None captures all current history, including later compaction facts.
        turn_id: Option<String>,
        side_conversation: bool,
    },
    EmptySideConversation,
    Revision {
        turn_id: String,
    },
}

/// The complete stable operation identity, not just its resolved history cut.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyRequest {
    pub source_session_id: String,
    pub target_session_id: String,
    pub expected_source_revision: u64,
    pub purpose: CopyPurpose,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchOrigin {
    pub parent_session_id: String,
    pub turn_id: Option<String>,
}

/// Flattened provenance remains readable after the source catalog is removed.
/// No parent lookup grants execution, workspace or plugin management authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Lineage {
    Branch {
        origin: BranchOrigin,
    },
    Revision {
        root_session_id: String,
        parent_session_id: String,
        turn_id: String,
        index: u64,
        branch: Option<BranchOrigin>,
    },
}

impl Lineage {
    pub fn branch(&self) -> Option<&BranchOrigin> {
        match self {
            Self::Branch { origin } => Some(origin),
            Self::Revision { branch, .. } => branch.as_ref(),
        }
    }
}

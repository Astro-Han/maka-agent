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

use crate::{archive::valid_projection_digest, context::ModelRequestContext};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

/// Continuation settings, not credentials or process-local resources.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffExecution {
    /// Admission projection, before any successor-side automatic compaction.
    pub replay: crate::continuation::ReplayEvidence,
    pub context: Option<ModelRequestContext>,
    /// Provider-specific options intentionally retain their extensible JSON shape.
    pub provider_options: Value,
    pub main_output_limit: Option<u64>,
    pub supports_vision: bool,
    pub tools: HandoffTools,
    pub compaction_attempted: bool,
    /// Manual continuation's stable-cut policy, absent for ordinary conversation.
    /// Physical handoff must not introduce or discard this projection policy.
    pub replay_base: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffTools {
    /// Covers definitions, nesting, execution semantics and discovery mode.
    /// It does not promise persistence of a handler's executable implementation.
    pub catalog_digest: String,
    pub loaded: BTreeSet<String>,
}

impl HandoffExecution {
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        self.replay.validate()?;
        if !valid_projection_digest(&self.tools.catalog_digest)
            || self
                .main_output_limit
                .is_some_and(|n| n == 0 || n > 10_000_000_000)
            || self.tools.loaded.len() > 128
            || self
                .tools
                .loaded
                .iter()
                .any(|name| name.is_empty() || name.len() > 128)
        {
            return Err("invalid handoff execution settings");
        }
        if let Some(context) = &self.context {
            context.validate()?;
        }
        Ok(())
    }
}

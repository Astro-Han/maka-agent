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

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

/// A continuation binds to one catalog revision. A conflict requires a fresh first page.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct List {
    pub revision: Option<String>,
    pub cursor: Option<String>,
}
impl List {
    pub fn validate(&self) -> Result<(), crate::execution::CommandError> {
        if self
            .revision
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > 128)
            || self
                .cursor
                .as_ref()
                .is_some_and(|value| value.is_empty() || value.len() > 128)
            || (self.cursor.is_some() && self.revision.is_none())
        {
            return Err(crate::execution::CommandError::Invalid(
                "invalid Session catalog continuation".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Summary {
    pub session: super::View,
    pub labels: Vec<String>,
    pub updated_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Page {
    pub revision: String,
    pub entries: Vec<Summary>,
    pub next_cursor: Option<String>,
}

/// Read-only metadata, never execution authority or conversation content.
/// Agent calls see only their Session; independent calls require ReadSessions consent.
pub trait Queries: Send + Sync {
    fn list(
        &self,
        call: crate::call::Scope,
        input: List,
    ) -> BoxFuture<'_, Result<Page, crate::execution::CommandError>>;
}

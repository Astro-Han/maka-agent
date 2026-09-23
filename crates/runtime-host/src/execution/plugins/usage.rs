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

use super::{Error, Executions, storage};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use maka_plugins::{
    authorization::{Boundary, Capability},
    call::Scope,
    usage::{Filter, Page, Read, Summary},
};
use serde::{Deserialize, Serialize};

const MAX_NUMBER: u64 = (1 << 53) - 1;

/// Carries the entire frozen query, so continuations cannot accidentally reuse
/// an offset with a different filter. It never substitutes for authorization.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    generation: String,
    filter: Filter,
    through: u64,
    offset: u64,
}
impl Cursor {
    fn decode(value: &str, generation: &str) -> Result<Self, Error> {
        if value.len() > 8192 {
            return Err(Error::Invalid("oversized Usage cursor".into()));
        }
        let cursor: Self = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(value)
                .map_err(|_| invalid_cursor())?,
        )
        .map_err(|_| invalid_cursor())?;
        cursor.filter.validate()?;
        if cursor.generation != generation {
            return Err(Error::Conflict);
        }
        if cursor.through > MAX_NUMBER || cursor.offset > MAX_NUMBER {
            return Err(invalid_cursor());
        }
        Ok(cursor)
    }
    fn encode(&self) -> Result<String, Error> {
        let bytes = serde_json::to_vec(self).map_err(|_| invalid_cursor())?;
        let encoded = URL_SAFE_NO_PAD.encode(bytes);
        if encoded.len() > 8192 {
            return Err(invalid_cursor());
        }
        Ok(encoded)
    }
}
fn invalid_cursor() -> Error {
    Error::Invalid("invalid Usage cursor".into())
}

impl Executions {
    pub(crate) async fn plugin_usage_summary(
        &self,
        call: Scope,
        cursor: String,
    ) -> Result<Summary, Error> {
        let cursor = Cursor::decode(&cursor, self.interactions.epoch())?;
        self.check_usage_filter(&call, &cursor.filter).await?;
        let report = self
            .log
            .usage_summary(maka_event_log::usage::Query {
                from: cursor.filter.from,
                to: cursor.filter.to,
                session_id: cursor.filter.session_id.clone(),
                through: Some(cursor.through),
            })
            .await
            .map_err(usage_error)?;
        self.check_usage_filter(&call, &cursor.filter).await?;
        Ok(report.summary)
    }

    async fn check_usage_filter(&self, call: &Scope, filter: &Filter) -> Result<(), Error> {
        let mut checked = filter.clone();
        restrict(self.usage_boundary(call).await?, &mut checked)?;
        if &checked != filter {
            return Err(Error::Revoked);
        }
        Ok(())
    }

    async fn usage_boundary(&self, call: &Scope) -> Result<Boundary, Error> {
        if call.identity.agent().is_some() {
            self.plugin_execution_boundary(call).await
        } else {
            self.plugin_resource_boundary(call, Capability::ReadUsage)
                .await
        }
    }

    pub(crate) async fn plugin_usage_activity(
        &self,
        call: Scope,
        input: Read,
    ) -> Result<Page, Error> {
        let (mut filter, through, offset) = match input {
            Read::Start { filter } => (filter, None, 0),
            Read::Continue { cursor } => {
                let cursor = Cursor::decode(&cursor, self.interactions.epoch())?;
                (cursor.filter, Some(cursor.through), cursor.offset)
            }
        };
        filter.validate()?;
        let requested = filter.clone();
        restrict(self.usage_boundary(&call).await?, &mut filter)?;
        if through.is_some() && requested != filter {
            return Err(Error::Denied);
        }
        let result = self
            .log
            .usage_activity(
                maka_event_log::usage::Query {
                    from: filter.from,
                    to: filter.to,
                    session_id: filter.session_id.clone(),
                    through,
                },
                filter.activity.clone(),
                offset,
                100,
            )
            .await
            .map_err(usage_error)?;
        // Reading can yield to consent revocation or a Session boundary change.
        self.check_usage_filter(&call, &filter).await?;
        if result.through > MAX_NUMBER || result.total > MAX_NUMBER || offset > result.total {
            return Err(invalid_cursor());
        }
        let mut cursor = Cursor {
            generation: self.interactions.epoch().to_owned(),
            filter,
            through: result.through,
            offset,
        };
        let current = cursor.encode()?;
        let next_cursor = result
            .next_offset
            .map(|offset| {
                cursor.offset = offset;
                cursor.encode()
            })
            .transpose()?;
        let page = Page {
            cursor: current,
            next_cursor,
            attempts: result.attempts,
            total: result.total,
        };
        if serde_json::to_vec(&page)
            .map_err(|_| invalid_cursor())?
            .len()
            > 48 * 1024
        {
            return Err(Error::Invalid("Usage page exceeds capacity".into()));
        }
        Ok(page)
    }
}

fn restrict(boundary: Boundary, filter: &mut Filter) -> Result<(), Error> {
    match boundary {
        Boundary::Profile => Ok(()),
        Boundary::Session { boundary, .. } => {
            if filter
                .session_id
                .as_ref()
                .is_some_and(|id| id != &boundary.session_id)
            {
                return Err(Error::Denied);
            }
            filter.session_id = Some(boundary.session_id);
            Ok(())
        }
        Boundary::Workspace { .. } | Boundary::Directory { .. } => Err(Error::Denied),
    }
}

fn usage_error(error: maka_event_log::StoreError) -> Error {
    match error {
        maka_event_log::StoreError::InvalidTransition(message) => Error::Invalid(message),
        other => storage(other),
    }
}

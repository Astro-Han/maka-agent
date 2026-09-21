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

use super::{Error, Executions, SessionConfiguration, storage};
use maka_event_log::sessions::CatalogScope;
use maka_plugins::{
    authorization::{Boundary, Capability},
    call::Scope,
    session::catalog::{List, Page, Summary},
};

impl Executions {
    pub(crate) async fn plugin_session_catalog(
        &self,
        call: Scope,
        input: List,
    ) -> Result<Page, Error> {
        input.validate()?;
        let boundary = if call.identity.agent().is_some() {
            self.plugin_execution_boundary(&call).await?
        } else {
            self.plugin_resource_boundary(&call, Capability::ReadSessions)
                .await?
        };
        self.plugin_catalog_page(boundary, input).await
    }

    pub(super) async fn plugin_catalog_page(
        &self,
        boundary: Boundary,
        input: List,
    ) -> Result<Page, Error> {
        let scope = match boundary {
            Boundary::Profile => CatalogScope::Profile,
            Boundary::Session { boundary, .. } => CatalogScope::Session(boundary.session_id),
            Boundary::Workspace { workspace, .. } => CatalogScope::Workspace(workspace.host_cwd),
            Boundary::Directory { .. } => return Err(Error::Denied),
        };
        let page = self
            .log
            .scoped_sessions::<SessionConfiguration>(
                scope,
                input.revision.as_deref(),
                input.cursor.as_deref(),
                input.include_archived,
            )
            .await
            .map_err(|error| match error {
                maka_event_log::StoreError::RevisionConflict { .. } => Error::Conflict,
                other => storage(other),
            })?;
        Ok(Page {
            revision: page.revision,
            next_cursor: page.next_cursor,
            entries: page
                .sessions
                .into_iter()
                .map(|record| {
                    let session = record.configuration.plugin_view(record.id, record.revision);
                    Summary {
                        archived: record.archived,
                        session,
                        labels: record.configuration.labels,
                        updated_at: record.updated_at,
                        last_message_at: record
                            .execution
                            .and_then(|execution| execution.last_message)
                            .map(|message| message.recorded_at),
                    }
                })
                .collect(),
        })
    }
}

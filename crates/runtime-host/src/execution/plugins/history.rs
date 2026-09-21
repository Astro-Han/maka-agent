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
use maka_plugins::{
    authorization::{Boundary, Capability},
    call::Scope,
    session::{
        catalog,
        history::{Page, Read},
    },
};

impl Executions {
    async fn history_boundary(&self, call: &Scope) -> Result<Boundary, Error> {
        if call.identity.agent().is_some() {
            // Installation already trusts plugin code. Normal Agent recall is
            // profile-wide, like the original runtime, not a per-Session consent UI.
            self.plugin_execution_boundary(call).await?;
            Ok(Boundary::Profile)
        } else {
            self.plugin_resource_boundary(call, Capability::ReadHistory)
                .await
        }
    }

    pub(crate) async fn plugin_history_catalog(
        &self,
        call: Scope,
        input: catalog::List,
    ) -> Result<catalog::Page, Error> {
        input.validate()?;
        let boundary = self.history_boundary(&call).await?;
        let page = self.plugin_catalog_page(boundary, input).await?;
        self.history_boundary(&call).await?;
        Ok(page)
    }

    async fn check_history_target(&self, call: &Scope, session: &str) -> Result<(), Error> {
        let boundary = self.history_boundary(call).await?;
        let record = self
            .log
            .get_session::<SessionConfiguration>(session)
            .await
            .map_err(storage)?
            .ok_or(Error::NotFound)?;
        let allowed = match boundary {
            Boundary::Profile => true,
            Boundary::Session { boundary, .. } => boundary.session_id == session,
            Boundary::Workspace { workspace, .. } => {
                workspace.host_cwd == record.configuration.workspace.host_cwd
            }
            Boundary::Directory { .. } => false,
        };
        if allowed { Ok(()) } else { Err(Error::Denied) }
    }

    pub(crate) async fn plugin_history_read(
        &self,
        call: Scope,
        input: Read,
    ) -> Result<Page, Error> {
        input.validate()?;
        self.check_history_target(&call, &input.session_id).await?;
        let high_water = *self.log.subscribe_commits().borrow();
        let through = input.through.unwrap_or(high_water);
        if through > high_water {
            return Err(Error::Invalid("history fence is in the future".into()));
        }
        let result = if !self
            .log
            .prepare_transcript(&input.session_id, through, 32)
            .await
            .map_err(storage)?
        {
            Page::Preparing { through }
        } else {
            self.log
                .history_text(&input.session_id, through, input.cursor)
                .await
                .map_err(storage)?
        };
        // The read and any index work may have yielded to revocation or deletion.
        self.check_history_target(&call, &input.session_id).await?;
        Ok(result)
    }
}

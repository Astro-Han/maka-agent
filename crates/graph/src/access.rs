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

use crate::Error;
use maka_plugins::{
    authorization::{Capability, Id, Target},
    execution::{Access as Executions, Commands},
    storage::{Data, Mutation, Store},
};
use std::sync::Arc;

/// Graph-owned consent references; only Host can resolve them into authority.
pub struct Access {
    pub storage: Arc<dyn Store>,
    pub executions: Arc<dyn Executions>,
    pub authorizations: Arc<dyn maka_plugins::authorization::Access>,
}
impl Access {
    pub async fn remember(&self, session: &str, id: Id) -> Result<(), Error> {
        let authorized = self.authorizations.open(id).await?;
        let valid = matches!(&authorized.grant.request.target, Target::Session { session_id } if session_id == session)
            && authorized
                .grant
                .request
                .capabilities
                .contains(&Capability::Executions);
        authorized
            .call
            .finish()
            .await
            .map_err(|error| Error::Invalid(error.to_string()))?;
        if !valid {
            return Err(Error::Invalid(
                "Graph consent must authorize this Session's executions".into(),
            ));
        }
        let commands = self.executions.restore(id).await?;
        commands.session(session.into()).await?;
        let key = key(session);
        let previous = self.storage.read(key.clone()).await?;
        self.storage
            .batch(vec![Mutation {
                key,
                expected_revision: previous.map(|record| record.revision),
                data: Data::Present(
                    serde_json::to_value(id).map_err(|error| Error::Invalid(error.to_string()))?,
                ),
            }])
            .await?;
        Ok(())
    }

    pub async fn commands(&self, session: &str) -> Result<Arc<dyn Commands>, Error> {
        let record = self
            .storage
            .read(key(session))
            .await?
            .ok_or(Error::Host(maka_plugins::execution::CommandError::Denied))?;
        let Data::Present(value) = record.data else {
            return Err(Error::Host(maka_plugins::execution::CommandError::Denied));
        };
        let id =
            serde_json::from_value(value).map_err(|error| Error::Invalid(error.to_string()))?;
        let commands = self.executions.restore(id).await?;
        commands.session(session.into()).await?;
        Ok(commands)
    }
}
fn key(session: &str) -> String {
    format!(
        "consent:{}",
        maka_runtime::artifact::content_digest(session.as_bytes())
    )
}

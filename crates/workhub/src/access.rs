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

use crate::{Error, Repository, invalid, repository::digest};
use maka_plugins::{
    authorization::{Authorized, Capability, Id, Target},
    execution::{Access as Executions, CommandError, Commands},
};
use std::sync::Arc;

/// Remembered consent is only a reference. Host rechecks its owner and current grant on every use.
pub struct Access {
    pub repository: Arc<Repository>,
    pub executions: Arc<dyn Executions>,
    pub authorizations: Arc<dyn maka_plugins::authorization::Access>,
}
impl Access {
    pub async fn remember(&self, id: Id) -> Result<Target, Error> {
        let authorized = self.authorizations.open(id).await?;
        let target = authorized.grant.request.target.clone();
        let required = if matches!(target, Target::Profile) {
            Capability::ReadSessions
        } else {
            Capability::Executions
        };
        let valid = authorized.grant.request.capabilities.contains(&required)
            && !matches!(target, Target::Directory { .. });
        authorized.call.finish().await.map_err(invalid)?;
        if !valid {
            return Err(invalid(
                "WorkHub consent must authorize task discovery or execution",
            ));
        }
        let key = key(&target)?;
        let expected = self
            .repository
            .read::<Id>(&key)
            .await?
            .map(|(revision, _)| revision);
        self.repository.put(&key, expected, &id).await?;
        Ok(target)
    }

    pub async fn open(&self, target: &Target) -> Result<Authorized, Error> {
        let id = self.id(target).await?;
        let authorized = self.authorizations.open(id).await?;
        if authorized.grant.request.target != *target {
            authorized.call.finish().await.map_err(invalid)?;
            return Err(Error::Conflict);
        }
        Ok(authorized)
    }

    pub async fn commands(&self, target: &Target) -> Result<Arc<dyn Commands>, Error> {
        Ok(self.executions.restore(self.id(target).await?).await?)
    }

    pub(super) async fn remembered(&self, target: &Target) -> Result<Option<Id>, Error> {
        Ok(self.repository.read(&key(target)?).await?.map(|(_, id)| id))
    }

    async fn id(&self, target: &Target) -> Result<Id, Error> {
        self.repository
            .read(&key(target)?)
            .await?
            .map(|(_, id)| id)
            .ok_or(Error::Execution(CommandError::Denied))
    }
}
fn key(target: &Target) -> Result<String, Error> {
    Ok(format!("consents/{}", digest(target)?))
}

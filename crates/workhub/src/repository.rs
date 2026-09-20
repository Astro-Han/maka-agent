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

use crate::{Error, invalid};
use maka_plugins::storage::{Data, Mutation, Record, Store, StoreError};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::sync::Arc;

/// Plugin-owned data, not another execution log. CAS coordinates Entries sharing a namespace.
pub struct Repository {
    storage: Arc<dyn Store>,
}
impl Repository {
    pub fn new(storage: Arc<dyn Store>) -> Self {
        Self { storage }
    }

    pub(crate) async fn read<T: DeserializeOwned>(
        &self,
        key: &str,
    ) -> Result<Option<(u64, T)>, Error> {
        self.storage.read(key.into()).await?.map(decode).transpose()
    }
    pub(crate) async fn put<T: Serialize>(
        &self,
        key: &str,
        expected: Option<u64>,
        value: &T,
    ) -> Result<(), Error> {
        self.commit(vec![mutation(key, expected, value)?]).await
    }
    pub(crate) async fn scan<T: DeserializeOwned>(
        &self,
        prefix: &str,
        after: Option<String>,
    ) -> Result<(Vec<T>, Option<String>), Error> {
        let page = self
            .storage
            .scan(maka_plugins::storage::Scan {
                prefix: prefix.into(),
                after,
            })
            .await?;
        let values = page
            .entries
            .into_iter()
            .map(|entry| decode(entry.record).map(|(_, value)| value))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((values, page.next_after))
    }
    pub(crate) async fn pending(&self) -> Result<Vec<Pending>, Error> {
        Ok(self
            .read::<Vec<Pending>>("pending")
            .await?
            .map(|(_, work)| work)
            .unwrap_or_default())
    }
    /// Intent and its recovery index commit together. Completed history is never
    /// scanned to discover unfinished work; conflicts retry the domain decision.
    pub(crate) async fn transition(
        &self,
        mut mutations: Vec<Mutation>,
        pending: Pending,
        unfinished: bool,
    ) -> Result<(), Error> {
        let previous = self.read::<Vec<Pending>>("pending").await?;
        let revision = previous.as_ref().map(|(revision, _)| *revision);
        let mut work = previous.map(|(_, work)| work).unwrap_or_default();
        if unfinished {
            if !work.contains(&pending) {
                if work.len() >= 256 {
                    return Err(invalid("WorkHub has 256 unfinished decisions"));
                }
                work.push(pending);
            }
        } else {
            work.retain(|item| item != &pending);
        }
        mutations.push(mutation("pending", revision, &work)?);
        self.commit(mutations).await
    }
    pub(crate) async fn commit(&self, mutations: Vec<Mutation>) -> Result<(), Error> {
        match self.storage.batch(mutations).await {
            Ok(_) => Ok(()),
            Err(StoreError::Conflict { .. }) => Err(Error::Contended),
            Err(error) => Err(error.into()),
        }
    }
}
pub(crate) fn mutation<T: Serialize>(
    key: &str,
    revision: Option<u64>,
    value: &T,
) -> Result<Mutation, Error> {
    Ok(Mutation {
        key: key.into(),
        expected_revision: revision,
        data: Data::Present(serde_json::to_value(value).map_err(invalid)?),
    })
}
fn decode<T: DeserializeOwned>(record: Record) -> Result<(u64, T), Error> {
    match record.data {
        Data::Present(value) => Ok((
            record.revision,
            serde_json::from_value(value).map_err(invalid)?,
        )),
        Data::Deleted => Err(invalid("unexpected deleted business record")),
    }
}
pub(crate) fn digest(value: &impl Serialize) -> Result<String, Error> {
    use sha2::{Digest, Sha256};
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(invalid)?)
    ))
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "operationId", rename_all = "snake_case")]
pub(crate) enum Pending {
    Decision(String),
    Route(String),
    Control(String),
}

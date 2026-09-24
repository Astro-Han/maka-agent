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

use crate::{Client, ClientError, RequestFailure};
use maka_protocol::{
    Operation,
    model_provider::{self, Entry, Page, Query, Scope},
};
use serde_json::json;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct ProviderDirectory {
    pub revision: u64,
    pub entries: Vec<Entry>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderDirectoryError {
    #[error(transparent)]
    Request(#[from] RequestFailure),
    #[error("Provider directory kept changing while it was read")]
    Unstable,
}

impl Client {
    /// A complete publication revision. A concurrent replacement discards all
    /// collected pages; it never returns a mixture of provider generations.
    pub async fn provider_directory(
        &self,
        scope: Scope,
    ) -> Result<ProviderDirectory, ProviderDirectoryError> {
        for attempt in 0..8 {
            let mut query = Query {
                scope: scope.clone(),
                after: None,
                revision: None,
            };
            let mut entries = Vec::new();
            loop {
                match self.model_providers(query).await? {
                    Page::RevisionChanged { .. } => break,
                    Page::Page {
                        revision,
                        entries: page,
                        next,
                    } => {
                        entries.extend(page);
                        let Some(after) = next else {
                            return Ok(ProviderDirectory { revision, entries });
                        };
                        query = Query {
                            scope: scope.clone(),
                            after: Some(after),
                            revision: Some(revision),
                        };
                    }
                }
            }
            if attempt < 7 {
                tokio::time::sleep(Duration::from_millis((4 << attempt).min(64))).await;
            }
        }
        Err(ProviderDirectoryError::Unstable)
    }

    pub async fn model_providers(&self, query: Query) -> Result<Page, RequestFailure> {
        model_provider::validate_query(&query).map_err(|error| {
            RequestFailure::NotDispatched(ClientError::Protocol(error.to_string()))
        })?;
        let value = self
            .request(
                Operation::ModelProviderCatalogQuery,
                json!({
                    "scope":query.scope, "after":query.after, "revision":query.revision,
                }),
            )
            .await?;
        model_provider::decode_page(&value)
            .and_then(|page| {
                model_provider::assert_page(&query, &page)?;
                Ok(page)
            })
            .map_err(|error| {
                self.disconnect();
                RequestFailure::Unknown(ClientError::Protocol(error.to_string()))
            })
    }
}

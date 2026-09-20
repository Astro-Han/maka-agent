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

use super::NativeOperators;
use crate::coordinator::Operators;
use futures_util::future::BoxFuture;

impl Operators for NativeOperators {
    fn active(&self) -> BoxFuture<'_, Result<bool, crate::Error>> {
        Box::pin(async move {
            let authorized = match self.commands.session(self.root.clone()).await {
                Ok(_) => true,
                Err(
                    maka_plugins::execution::CommandError::Revoked
                    | maka_plugins::execution::CommandError::Denied
                    | maka_plugins::execution::CommandError::NotFound,
                ) => false,
                Err(error) => return Err(error.into()),
            };
            let current = self
                .repository
                .current(&self.root)
                .await
                .map_err(|error| crate::Error::Persistence(error.to_string()))?;
            // Epoch identity, not the Session default, owns asynchronous work.
            // A one-turn override must survive yield, reactivation and restart.
            let active = authorized
                && current.is_some_and(|control| control.epoch.graph_id == self.graph_id);
            if !active {
                let _invalidating = self.revision.invalidate().await;
                self.cancel.cancel();
            }
            Ok(active)
        })
    }
    fn provision(
        &self,
        epoch: &crate::Epoch,
        operator: &crate::OperatorId,
        target: &crate::schedule::Target,
    ) -> BoxFuture<'_, Result<maka_plugins::execution::ChildSession, crate::Error>> {
        let (epoch, operator, target) = (epoch.clone(), operator.clone(), target.clone());
        Box::pin(async move {
            let key = format!("operator:{}:{operator}", epoch.graph_id);
            let request = match self.storage.read(key.clone()).await.map_err(persistence)? {
                Some(record) => decode(record)?,
                None => {
                    let parent = self.commands.session(epoch.root_session_id.clone()).await?;
                    let request = self
                        .definitions
                        .resolve(&target, &parent, key.clone(), epoch.root_session_id.clone())
                        .await
                        .map_err(crate::Error::Invalid)?;
                    let result = self
                        .storage
                        .batch(vec![maka_plugins::storage::Mutation {
                            key: key.clone(),
                            expected_revision: None,
                            data: maka_plugins::storage::Data::Present(
                                serde_json::to_value(&request).map_err(persistence)?,
                            ),
                        }])
                        .await;
                    match result {
                        Ok(_) => request,
                        Err(maka_plugins::storage::StoreError::Conflict { .. }) => decode(
                            self.storage
                                .read(key.clone())
                                .await
                                .map_err(persistence)?
                                .ok_or_else(|| {
                                    crate::Error::Persistence(
                                        "operator reservation disappeared".into(),
                                    )
                                })?,
                        )?,
                        Err(error) => return Err(persistence(error)),
                    }
                }
            };
            if request.operation_id != key || request.parent_session_id != epoch.root_session_id {
                return Err(persistence("operator reservation identity changed"));
            }
            Ok(self.commands.create_child(request).await?)
        })
    }
}

fn decode(
    record: maka_plugins::storage::Record,
) -> Result<maka_plugins::execution::CreateChild, crate::Error> {
    match record.data {
        maka_plugins::storage::Data::Present(value) => {
            serde_json::from_value(value).map_err(persistence)
        }
        maka_plugins::storage::Data::Deleted => Err(crate::Error::Persistence(
            "operator reservation was deleted".into(),
        )),
    }
}
fn persistence(error: impl ToString) -> crate::Error {
    crate::Error::Persistence(error.to_string())
}

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

use super::{Repository, Saved};
use crate::Error;
use maka_plugins::{
    authorization::{Capability, Request as Authorization, Target},
    execution::{Access, CommandError, Commands, CreateRoot},
    remote::Views,
    session::import::{Command, ImportState, Receipt},
};
use maka_runtime::import::MAX_RECORD_BYTES;
use uuid::Uuid;

impl Repository {
    /// Reauthorize the saved destination through the real caller on every
    /// attempt. Neither a saved intent nor an unrelated Commands handle grants it.
    pub async fn deliver(
        &self,
        id: Uuid,
        views: &dyn Views,
        access: &dyn Access,
    ) -> Result<Receipt, Error> {
        self.authorized(id, views, access, Action::Deliver).await
    }

    pub async fn abandon(
        &self,
        id: Uuid,
        views: &dyn Views,
        access: &dyn Access,
    ) -> Result<Receipt, Error> {
        self.authorized(id, views, access, Action::Abandon).await
    }

    async fn authorized(
        &self,
        id: Uuid,
        views: &dyn Views,
        access: &dyn Access,
        action: Action,
    ) -> Result<Receipt, Error> {
        let saved = self
            .get(id)
            .await?
            .ok_or(Error::Invalid("unknown import intent"))?;
        let owned = views
            .authorize(Authorization {
                operation_id: id,
                title: "Import conversation".into(),
                target: Target::Workspace {
                    workspace: saved.intent.request.workspace.clone(),
                    sandbox_mode: saved.intent.request.settings.sandbox_mode,
                },
                capabilities: [Capability::Executions].into(),
            })
            .await?;
        let result = async {
            let commands = access.acquire(owned.scope()).await?;
            self.apply(saved, commands.as_ref(), action).await
        }
        .await;
        owned.finish().await.map_err(|error| {
            Error::Remote(match error {
                maka_runtime::tools::ToolError::Persistence(message)
                | maka_runtime::tools::ToolError::OutcomeUnknown(message) => {
                    maka_plugins::remote::Error::OutcomeUnknown(message)
                }
                maka_runtime::tools::ToolError::CleanupUnconfirmed(_) => {
                    maka_plugins::remote::Error::CleanupUnconfirmed
                }
                other => maka_plugins::remote::Error::Provider(other.to_string()),
            })
        })?;
        result
    }

    async fn apply(
        &self,
        saved: Saved,
        commands: &dyn Commands,
        action: Action,
    ) -> Result<Receipt, Error> {
        let id = saved.intent.request.operation_id;
        let operation_id = id.to_string();
        let mut loaded = None;
        let inspected = commands
            .import_session(Command::Inspect {
                operation_id: operation_id.clone(),
            })
            .await;
        let receipt = match inspected {
            Ok(receipt) => receipt,
            Err(CommandError::NotFound) if saved.intent.receipt.is_none() => {
                let transcript = self.transcript(&saved).await?;
                let source = transcript.source.clone();
                loaded = Some(transcript);
                commands
                    .import_session(Command::Begin {
                        root: Box::new(CreateRoot {
                            operation_id: operation_id.clone(),
                            managed: false,
                            name: saved.intent.title.clone(),
                            settings: saved.intent.request.settings.clone(),
                        }),
                        source,
                    })
                    .await?
            }
            Err(error) => return Err(error.into()),
        };
        if receipt.progress.state != ImportState::Collecting {
            return self.settle(saved, receipt).await;
        }
        if let Action::Abandon = action {
            let receipt = commands
                .import_session(Command::Abandon { operation_id })
                .await?;
            return self.settle(saved, receipt).await;
        }
        let transcript = match loaded {
            Some(transcript) => transcript,
            None => self.transcript(&saved).await?,
        };
        let position = usize::try_from(receipt.progress.records)
            .map_err(|_| Error::Invalid("invalid import position"))?;
        if position > transcript.records.len() {
            return Err(Error::Conflict);
        }
        // Each append is atomic. Start from the observed committed suffix, not
        // from a locally remembered count or the source's current contents.
        let mut position = position;
        let mut remaining = transcript.records.into_iter().skip(position).peekable();
        while remaining.peek().is_some() {
            let mut records = Vec::with_capacity(8);
            let mut bytes = 2; // JSON array delimiters.
            while let Some(record) = remaining.peek() {
                let size = serde_json::to_vec(record)
                    .map_err(|_| Error::Invalid("invalid normalized record"))?
                    .len()
                    + usize::from(!records.is_empty());
                if records.len() == 8 || bytes + size > MAX_RECORD_BYTES {
                    break;
                }
                bytes += size;
                records.push(remaining.next().expect("peeked record"));
            }
            if records.is_empty() {
                return Err(Error::Invalid("normalized record exceeds append budget"));
            }
            let count = records.len();
            commands
                .import_session(Command::Append {
                    operation_id: operation_id.clone(),
                    position: position as u64,
                    records,
                })
                .await?;
            position += count;
        }
        let receipt = commands
            .import_session(Command::Publish {
                operation_id,
                records: saved.intent.records as u64,
            })
            .await?;
        self.settle(saved, receipt).await
    }

    async fn settle(&self, saved: Saved, receipt: Receipt) -> Result<Receipt, Error> {
        self.complete(saved, receipt.clone()).await?;
        Ok(receipt)
    }
}

enum Action {
    Deliver,
    Abandon,
}

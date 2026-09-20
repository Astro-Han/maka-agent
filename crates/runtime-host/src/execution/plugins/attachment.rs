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

use super::{BoundCommands, Commands, Error, storage};
use maka_plugins::execution::CopyAttachment;
use maka_runtime::attachment::{AttachmentRef, StorageRef};
use std::sync::Arc;

impl BoundCommands {
    pub(super) async fn copy_attachment_from(
        &self,
        source: Arc<dyn Commands>,
        request: CopyAttachment,
    ) -> Result<AttachmentRef, Error> {
        request
            .validate()
            .map_err(|error| Error::Invalid(error.to_string()))?;
        // Commands are embedding-issued capabilities, not plugin-provided Services.
        // Reject other embeddings and synthetic implementations, even if they report
        // matching Session names. Native and JS callers use these same handles.
        let source = (&*source as &dyn std::any::Any)
            .downcast_ref::<BoundCommands>()
            .ok_or(Error::Denied)?
            .clone();
        let host = self.executions()?;
        if !Arc::ptr_eq(&host, &source.executions()?) {
            return Err(Error::Denied);
        }
        let gate = host.interactions.own_admission().await;
        let source_lease = source.context.admit().map_err(|_| Error::Revoked)?;
        let target_lease = self.context.admit().map_err(|_| Error::Revoked)?;
        let StorageRef::SessionFile { session_id, .. } = &request.attachment.storage_ref else {
            return Err(Error::Denied);
        };
        source.authorize(&host, session_id).await?;
        self.authorize(&host, &request.target_session_id).await?;
        let namespace = self.namespace.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        let worker = host.clone();
        host.workers.spawn(async move {
            let result = worker
                .log
                .copy_plugin_attachment(&namespace, request)
                .await
                .map_err(|error| {
                    if matches!(
                        error,
                        maka_event_log::StoreError::CommitUnknown(_)
                            | maka_event_log::StoreError::OperationUnknown
                    ) {
                        worker.begin_drain();
                    }
                    storage(error)
                });
            drop(gate);
            drop(source_lease);
            drop(target_lease);
            let _ = send.send(result);
        });
        receive
            .await
            .map_err(|_| Error::OutcomeUnknown("attachment copy owner disappeared".into()))?
    }
}

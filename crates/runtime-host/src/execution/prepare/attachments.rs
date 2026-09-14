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

use super::{Code, Result, failure, internal};
use maka_event_log::EventLog;
use maka_runtime::{
    attachment::{AttachmentRef, StorageRef},
    interaction::entity_id,
};

/// Admission checks canonical descriptors, never filesystem authority. Consumers
/// resolve the same Session-scoped artifact again when they need its bytes.
pub(super) async fn validate(
    log: &EventLog,
    session_id: &str,
    attachments: &[AttachmentRef],
) -> Result<()> {
    for attachment in attachments {
        let StorageRef::SessionFile {
            session_id: attachment_session,
            relative_path,
        } = &attachment.storage_ref
        else {
            return Err(failure(
                Code::OperationConflict,
                "Hosted Turn attachments must use a Session Artifact reference",
            ));
        };
        if attachment_session != session_id {
            return Err(failure(
                Code::OperationConflict,
                "Attachment belongs to a different Session",
            ));
        }
        if entity_id(relative_path).is_err() {
            return Err(failure(
                Code::OperationConflict,
                "Attachment Artifact was not found",
            ));
        }
        let record = log
            .get_artifact(session_id, relative_path)
            .await
            .map_err(internal)?
            .record
            .ok_or_else(|| failure(Code::OperationConflict, "Attachment Artifact was not found"))?;
        record
            .validate_attachment(attachment)
            .map_err(|message| failure(Code::OperationConflict, message))?;
    }
    Ok(())
}

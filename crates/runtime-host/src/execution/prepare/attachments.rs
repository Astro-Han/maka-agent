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
    artifact::{Artifact, ArtifactKind},
    attachment::{AttachmentKind, AttachmentRef, StorageRef},
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
        validate_descriptor(attachment, &record)
            .map_err(|message| failure(Code::OperationConflict, message))?;
    }
    Ok(())
}

fn validate_descriptor(
    attachment: &AttachmentRef,
    record: &Artifact,
) -> std::result::Result<(), &'static str> {
    if record.name != attachment.name
        || record.mime_type.as_deref() != Some(attachment.mime_type.as_str())
        || record.size_bytes != attachment.bytes
    {
        return Err("Attachment metadata does not match its canonical Artifact");
    }
    let kind = AttachmentKind::from_metadata(&attachment.mime_type, &record.name);
    if attachment.kind != kind
        || (kind == AttachmentKind::Image) != (record.kind == ArtifactKind::Image)
        || (kind == AttachmentKind::Pdf) != (record.kind == ArtifactKind::Pdf)
    {
        return Err("Attachment kind does not match its canonical Artifact");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use maka_runtime::artifact::ArtifactSource;

    #[test]
    fn canonical_metadata_and_bidirectional_modality_are_required_without_source_restriction() {
        let attachment = AttachmentRef {
            kind: AttachmentKind::Image,
            name: "picture.png".into(),
            mime_type: "image/png".into(),
            bytes: 4,
            storage_ref: StorageRef::SessionFile {
                session_id: "session".into(),
                relative_path: "artifact".into(),
            },
        };
        let mut record = Artifact {
            id: "artifact".into(),
            session_id: "session".into(),
            turn_id: "turn".into(),
            created_at: 1,
            name: attachment.name.clone(),
            kind: ArtifactKind::Image,
            size_bytes: attachment.bytes,
            mime_type: Some(attachment.mime_type.clone()),
            source: ArtifactSource::ToolResultProjection,
            summary: None,
        };
        assert!(validate_descriptor(&attachment, &record).is_ok());
        for changed in [
            AttachmentRef {
                name: "other.png".into(),
                ..attachment.clone()
            },
            AttachmentRef {
                mime_type: "image/jpeg".into(),
                ..attachment.clone()
            },
            AttachmentRef {
                bytes: 5,
                ..attachment.clone()
            },
            AttachmentRef {
                kind: AttachmentKind::Other,
                ..attachment.clone()
            },
        ] {
            assert!(validate_descriptor(&changed, &record).is_err());
        }
        record.kind = ArtifactKind::File;
        assert!(validate_descriptor(&attachment, &record).is_err());
        let mut text = attachment;
        text.mime_type = "text/plain".into();
        text.kind = AttachmentKind::Other;
        record.mime_type = Some(text.mime_type.clone());
        assert!(validate_descriptor(&text, &record).is_ok());
        for kind in [ArtifactKind::Image, ArtifactKind::Pdf] {
            record.kind = kind;
            assert!(validate_descriptor(&text, &record).is_err());
        }
    }
}

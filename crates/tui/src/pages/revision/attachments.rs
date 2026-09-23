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

use super::*;
use crate::pages::attachments::Saved;

impl State {
    pub(crate) fn file_position(&self, session: &str, input: &str) -> Option<(usize, usize)> {
        let saved = self
            .saved
            .as_ref()
            .filter(|s| s.copy.target_session_id == session)?;
        Some((
            saved
                .inputs
                .iter()
                .position(|i| i.original.message_id == input)?
                + 1,
            saved.inputs.len(),
        ))
    }
    pub(crate) fn files(&self, session: &str, input: &str) -> Option<&[Saved]> {
        let saved = self
            .saved
            .as_ref()
            .filter(|s| s.copy.target_session_id == session)?;
        Some(
            &saved
                .inputs
                .iter()
                .find(|i| i.original.message_id == input)?
                .files,
        )
    }
    pub(crate) fn files_mut(&mut self, session: &str, input: &str) -> Option<&mut Vec<Saved>> {
        let saved = self
            .saved
            .as_mut()
            .filter(|s| s.copy.target_session_id == session)?;
        Some(
            &mut saved
                .inputs
                .iter_mut()
                .find(|i| i.original.message_id == input)?
                .files,
        )
    }
    pub(crate) fn files_editable(&self, session: &str, input: &str) -> bool {
        self.phase == Phase::Editing
            && self.pending.is_none()
            && self.requested.is_none()
            && !self.confirm_discard
            && self.files(session, input).is_some()
    }
    pub(crate) fn uploadable(&self, root: &str, session: &str, input: &str) -> bool {
        self.saved
            .as_ref()
            .is_some_and(|s| s.root == root && s.stage == Stage::Attachments)
            && self.uploading
            && self.files(session, input).is_some()
    }
    pub(crate) fn file_capacity(&self, session: &str, input: &str) -> usize {
        let Some(saved) = self.saved.as_ref().filter(|s| {
            s.copy.target_session_id == session
                && s.inputs.iter().any(|i| i.original.message_id == input)
        }) else {
            return 0;
        };
        let other = saved
            .inputs
            .iter()
            .map(|i| {
                i.message().content.attachments.iter().flatten().count()
                    + if i.original.message_id == input {
                        0
                    } else {
                        i.files.len()
                    }
            })
            .sum::<usize>();
        8usize.saturating_sub(other)
    }
}

impl App {
    pub(super) fn resume_revision_uploads(&mut self) {
        let Some(saved) = &self.revision.saved else {
            return;
        };
        if saved.stage != Stage::Attachments {
            return;
        }
        let session = saved.copy.target_session_id.clone();
        let files = saved
            .inputs
            .iter()
            .flat_map(|input| {
                input
                    .files
                    .iter()
                    .filter(|file| file.attachment.is_none())
                    .map(|file| (input.original.message_id.clone(), file.id.clone()))
            })
            .collect();
        self.revision.uploading = true;
        self.revision.phase = Phase::Uploading;
        self.revision.error = None;
        self.queue_revision_files(&session, files);
    }
    pub(crate) fn advance_revision_uploads(&mut self) {
        let state = &mut self.revision;
        if !state.uploading || self.closing {
            return;
        }
        let Some(saved) = &mut state.saved else {
            return;
        };
        if saved
            .inputs
            .iter()
            .flat_map(|i| &i.files)
            .any(|f| self.attachments.failed(&f.id))
        {
            state.uploading = false;
            state.phase = Phase::Ready;
            state.error = Some("attachments-upload-failed");
            self.attachments.retire(&saved.copy.target_session_id);
            return;
        }
        if saved.stage != Stage::Attachments
            || saved
                .inputs
                .iter()
                .flat_map(|i| &i.files)
                .any(|f| f.attachment.is_none())
        {
            return;
        }
        let mut batch = saved.mapped.clone().unwrap();
        for (input, message) in saved.inputs.iter().zip(&mut batch.messages) {
            if !input.files.is_empty() {
                message
                    .content
                    .attachments
                    .get_or_insert_with(Vec::new)
                    .extend(
                        input
                            .files
                            .iter()
                            .map(|file| file.attachment.clone().unwrap()),
                    );
            }
        }
        state.uploading = false;
        match maka_protocol::turn::decode_turn_batch_start_input(
            &serde_json::to_value(&batch).unwrap(),
        ) {
            Ok(batch) => {
                saved.batch = Some(batch.clone());
                saved.mapped = None;
                saved.stage = Stage::Batch;
                state.phase = Phase::Ready;
                state.requested = Some(Job::Start(batch));
            }
            Err(_) => {
                state.phase = Phase::Ready;
                state.error = Some("revision-invalid");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::attachments::{self, Manifest, Prepared, Read, Ticket};
    use maka_protocol::{
        session::sources,
        turn::{AttachmentRef, StorageRef},
    };
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;

    fn frame(app: &mut App) {
        Terminal::new(TestBackend::new(100, 30))
            .unwrap()
            .draw(|f| crate::view::draw(f, app))
            .unwrap();
    }
    fn sources(session: &str) -> sources::Output {
        sources::decode_output(&json!({"sessionId":session,"turnId":"turn","messages":[
            {"messageId":"one","content":{"text":"first"}}, {"messageId":"two","content":{"text":"second"}}
        ]})).unwrap()
    }
    fn manifest() -> Manifest {
        Manifest {
            name: "new.txt".into(),
            mime: "text/plain".into(),
            bytes: 1,
            digest: maka_protocol::artifact::content_digest(b"x"),
        }
    }
    fn reference(ticket: &Ticket) -> AttachmentRef {
        AttachmentRef {
            name: "new.txt".into(),
            mime_type: "text/plain".into(),
            bytes: 1,
            kind: maka_protocol::turn::AttachmentKind::Other,
            storage_ref: StorageRef::SessionFile {
                session_id: ticket.session.clone(),
                relative_path: maka_protocol::artifact::upload_artifact_id(
                    &ticket.session,
                    &ticket.id,
                ),
            },
        }
    }
    #[test]
    fn revision_uploads_wait_for_target_and_resume_without_mixing_inputs_or_replaying_admission() {
        let (mut app, basis) = crate::pages::branch::tests::fixture();
        app.apply(Action::Revision(Command::Open(basis)));
        let load = app.revision_request().unwrap();
        app.revision_completed(load, Ok(Output::Sources(sources("source"))));
        for index in 0..2 {
            frame(&mut app);
            app.apply(Action::Revision(Command::Select(index)));
            frame(&mut app);
            app.apply(Action::Revision(Command::Attachments));
            frame(&mut app);
            let browse = app.attachment_browse_request().unwrap();
            assert_eq!(
                browse.input.as_deref(),
                Some(if index == 0 { "one" } else { "two" })
            );
            app.attachment_browsed(
                browse,
                Ok(attachments::io::Listing::File("/local/new.txt".into())),
            );
            assert!(
                app.attachment_read_request().is_none(),
                "no read/upload before explicit copy"
            );
        }
        assert!(
            app.attachments.saved.is_empty(),
            "revision is not a composer draft"
        );
        app.revision.saved.as_mut().unwrap().inputs[0]
            .content
            .text
            .clear();
        let mut draft = app.revision.checkpoint().unwrap();
        // Put the cursor on the attachment-only input's actual text boundary.
        draft.view.positions.retain(|p| p.input != 0);
        draft.validate("root").unwrap();
        let target = draft.copy.target_session_id.clone();
        app.revision.restore(draft);
        app.apply(Action::Revision(Command::Resume));
        frame(&mut app);
        app.apply(Action::Revision(Command::Send));
        let copy = app.revision_request().unwrap();
        assert!(app.attachment_read_request().is_none());
        assert!(app.revision_after_checkpoint(&copy, &Ok(())));
        app.revision_completed(copy, Ok(Output::Sources(sources(&target))));
        let (first, _, _) = app.attachment_read_request().unwrap();
        assert_eq!(first.session, target);
        assert_eq!(first.input.as_deref(), Some("one"));
        let gate = app
            .attachment_prepared(
                first.clone(),
                Ok(Read::Prepared(Prepared {
                    manifest: manifest(),
                    bytes: b"x".to_vec(),
                })),
            )
            .unwrap();
        app.revision.checkpoint().unwrap().validate("root").unwrap();
        assert!(app.attachment_after_checkpoint(&gate, &Ok(())).is_some());
        let mut wrong = first.clone();
        wrong.input = Some("two".into());
        app.attachment_uploaded(wrong, Ok(reference(&first)));
        assert!(
            app.revision.files(&target, "one").unwrap()[0]
                .attachment
                .is_none()
        );
        app.attachment_uploaded(first.clone(), Ok(reference(&first)));
        let (second, _, _) = app.attachment_read_request().unwrap();
        assert_eq!(second.input.as_deref(), Some("two"));
        let gate = app
            .attachment_prepared(
                second.clone(),
                Ok(Read::Prepared(Prepared {
                    manifest: manifest(),
                    bytes: b"x".to_vec(),
                })),
            )
            .unwrap();
        app.attachment_after_checkpoint(&gate, &Ok(())).unwrap();
        app.attachment_uploaded(
            second.clone(),
            Err(attachments::Failure::Host("commit response lost".into())),
        );
        app.advance_revision_uploads();
        assert!(app.revision.phase == Phase::Ready);
        assert!(app.revision_request().is_none());
        let checkpoint = app.revision.checkpoint().unwrap();
        checkpoint.validate("root").unwrap();
        app.attachments = Default::default();
        app.revision = State::default();
        app.revision.restore(checkpoint);
        app.advance_revision_uploads();
        assert!(app.attachment_read_request().is_none());
        assert!(app.revision_request().is_none());
        app.apply(Action::Revision(Command::Resume));
        frame(&mut app);
        app.apply(Action::Revision(Command::Send));
        let (retry, saved, _) = app.attachment_read_request().unwrap();
        assert_eq!(retry.id, second.id);
        assert_eq!(retry.session, target);
        assert_eq!(saved.manifest, Some(manifest()));
        // The query branch can recover a committed reference without rereading a missing local file.
        app.attachment_prepared(retry.clone(), Ok(Read::Recovered(reference(&retry))));
        app.advance_revision_uploads();
        let start = app.revision_request().unwrap();
        let Job::Start(batch) = &start.job else {
            panic!("one batch");
        };
        assert_eq!(batch.messages.len(), 2);
        assert_eq!(batch.messages[0].content.text, "");
        assert_eq!(
            batch.messages[0].content.attachments,
            Some(vec![reference(&first)])
        );
        assert_eq!(
            batch.messages[1].content.attachments,
            Some(vec![reference(&retry)])
        );
        let frozen = app.revision.checkpoint().unwrap();
        frozen.validate("root").unwrap();
        let mut corrupt = frozen.clone();
        corrupt.batch.as_mut().unwrap().messages[1]
            .content
            .attachments = Some(vec![reference(&first)]);
        assert!(corrupt.validate("root").is_err());
        assert!(app.revision_after_checkpoint(&start, &Ok(())));
        assert!(!app.revision_after_checkpoint(&start, &Ok(())));
        app.revision = State::default();
        app.revision.restore(frozen);
        app.advance_revision_uploads();
        assert!(
            app.revision_request().is_none(),
            "restoring a batch never resends"
        );
    }
}

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
use crate::pages::references::Target;
use maka_protocol::turn::DirectoryReference;

impl State {
    pub(crate) fn directory_root(&self) -> Option<&str> {
        self.saved.as_ref().map(|s| s.root.as_str())
    }

    pub(crate) fn directory_target(&self) -> Option<Target> {
        let saved = self.saved.as_ref()?;
        Some(Target {
            session: saved.copy.target_session_id.clone(),
            input: Some(saved.inputs.get(self.selected)?.original.message_id.clone()),
        })
    }
    pub(crate) fn directories(&self, session: &str, input: &str) -> Option<&[DirectoryReference]> {
        Some(
            &self
                .saved
                .as_ref()
                .filter(|s| s.copy.target_session_id == session)?
                .inputs
                .iter()
                .find(|i| i.original.message_id == input)?
                .directories,
        )
    }
    pub(crate) fn directories_mut(
        &mut self,
        session: &str,
        input: &str,
    ) -> Option<&mut Vec<DirectoryReference>> {
        Some(
            &mut self
                .saved
                .as_mut()
                .filter(|s| s.copy.target_session_id == session)?
                .inputs
                .iter_mut()
                .find(|i| i.original.message_id == input)?
                .directories,
        )
    }
    pub(crate) fn directory_count(&self) -> usize {
        self.saved.as_ref().map_or(0, |s| {
            s.inputs
                .iter()
                .map(|i| {
                    i.message()
                        .content
                        .directory_references
                        .iter()
                        .flatten()
                        .count()
                })
                .sum()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::{
        manage::Command as Manage,
        references::tests::{complete, frame, selection},
    };
    use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    #[test]
    fn nested_directory_picker_keeps_revision_input_and_batch_limits_through_recovery() {
        let (mut app, basis) = crate::pages::branch::tests::fixture();
        app.apply(Action::Revision(Command::Open(basis)));
        let load = app.revision_request().unwrap();
        app.revision_completed(
            load,
            Ok(Output::Sources(super::super::tests::sources("source"))),
        );
        frame(&mut app, 80, 26);
        app.apply(Action::Revision(Command::Select(1)));
        frame(&mut app, 80, 26);
        app.apply(Action::Revision(Command::Directories));
        let request = selection(&mut app);
        // Outside dismisses only this child, never the revision underneath it.
        app.input(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }));
        assert!(app.revision.visible);
        assert!(!app.directory_reference_active());
        complete(&mut app, request, "/late");
        assert_eq!(app.revision.directory_count(), 0);
        frame(&mut app, 80, 26);
        app.apply(Action::Revision(Command::Directories));
        let request = selection(&mut app);
        complete(&mut app, request, "/actual/中文");
        assert!(app.revision.visible);
        let saved = app.revision.checkpoint().unwrap();
        assert!(saved.inputs[0].directories.is_empty());
        assert_eq!(saved.inputs[1].directories[0].path, "/actual/中文");
        saved.validate("root").unwrap();
        let mut wrong = saved.clone();
        wrong.inputs[1].directories[0].host_id = "other".into();
        assert!(wrong.validate("root").is_err());
        app.revision = State::default();
        app.revision.restore(saved);
        assert!(app.revision_request().is_none());
        app.apply(Action::Revision(Command::Resume));
        let session = app
            .revision
            .saved
            .as_ref()
            .unwrap()
            .copy
            .target_session_id
            .clone();
        // The cap applies to all inputs, not four per input.
        for index in 0..3 {
            app.revision
                .directories_mut(&session, "one")
                .unwrap()
                .push(DirectoryReference {
                    host_id: "root".into(),
                    path: format!("/dir/{index}"),
                });
        }
        frame(&mut app, 80, 26);
        app.apply(Action::Revision(Command::Directories));
        let query = app.directory_request().unwrap();
        app.directory_completed(
            query,
            Ok(maka_protocol::project::QueryResult::DirectoryRoots { roots: vec![] }),
        );
        frame(&mut app, 80, 26);
        assert!(!app.management_enabled(&Manage::Save));
        app.apply(Action::Manage(Manage::Close));
        let saved = app.revision.checkpoint().unwrap();
        let target = super::super::tests::sources(&session);
        let batch = super::super::draft::batch(&saved.inputs, &target, "turn-new").unwrap();
        assert_eq!(
            batch.messages[0]
                .content
                .directory_references
                .as_ref()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(
            batch.messages[1]
                .content
                .directory_references
                .as_ref()
                .unwrap()[0]
                .path,
            "/actual/中文"
        );
        assert!(
            app.directories.is_empty(),
            "revision additions never enter the composer or source session"
        );
    }
}

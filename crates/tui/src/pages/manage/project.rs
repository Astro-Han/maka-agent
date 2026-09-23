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

use super::{Command, Entity, Kind, Target, Ticket};
use crate::app::{Action, App, ConnectionState};
use maka_client::{Client, ClientError, RequestFailure};
use maka_protocol::project::{Mutation, Project};

pub(super) async fn execute(client: &Client, ticket: &Ticket) -> Result<Project, RequestFailure> {
    let input = match (&ticket.target.entity, ticket.kind) {
        (Entity::Registration, Kind::Register) => match &ticket.directory {
            Some(location) => Mutation::RegisterDirectory {
                root_id: location.root_id.clone(),
                segments: location.segments.clone(),
            },
            None => Mutation::Register {
                path: ticket.text.clone(),
                prefer: None,
            },
        },
        (Entity::Project { id }, Kind::Rename) => Mutation::Rename {
            project_id: id.clone(),
            name: ticket.text.clone(),
        },
        (Entity::Project { id }, Kind::Relink) => Mutation::Relink {
            project_id: id.clone(),
            path: ticket.text.clone(),
        },
        (Entity::Project { id }, Kind::Archive) => Mutation::Archive {
            project_id: id.clone(),
        },
        (Entity::Project { id }, Kind::Restore) => Mutation::Restore {
            project_id: id.clone(),
        },
        _ => {
            return Err(RequestFailure::NotDispatched(ClientError::Protocol(
                "Invalid project target".into(),
            )));
        }
    };
    client.mutate_project(input).await
}

impl App {
    pub fn register_project_action(&self) -> Option<Action> {
        let ConnectionState::Connected { root_id, epoch } = &self.connection else {
            return None;
        };
        Some(Action::Manage(Command::Open(
            Target {
                root: root_id.clone(),
                epoch: epoch.clone(),
                name: String::new(),
                entity: Entity::Registration,
            },
            Kind::Register,
        )))
    }
    pub(super) fn project_management_commands(
        &self,
        root: &str,
        epoch: &str,
    ) -> Vec<(Action, &'static str)> {
        let mut commands = self
            .register_project_action()
            .map(|a| (a, "project-register"))
            .into_iter()
            .collect::<Vec<_>>();
        if let Some(item) = self
            .projects
            .items
            .iter()
            .find(|item| Some(&item.id) == self.projects.selected.as_ref())
        {
            let target = Target {
                root: root.into(),
                epoch: epoch.into(),
                name: item.name.clone(),
                entity: Entity::Project {
                    id: item.id.clone(),
                },
            };
            for kind in [
                Kind::Locations,
                Kind::Rename,
                Kind::Relink,
                if item.archived {
                    Kind::Restore
                } else {
                    Kind::Archive
                },
            ] {
                commands.push((
                    Action::Manage(Command::Open(target.clone(), kind)),
                    kind.label(&target),
                ));
            }
        }
        commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Locale, LocalePreference,
        i18n::I18n,
        navigation::Route,
        pages::manage::{Entity, Updated},
    };
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn relink_requires_review_and_freezes_path_before_any_write() {
        let mut app = App::new(
            "/unused".into(),
            I18n::new(LocalePreference::Explicit(Locale::En), Locale::En),
        );
        app.connection = ConnectionState::Connected {
            root_id: "root".into(),
            epoch: "epoch".into(),
        };
        app.apply(Action::Visit(Route::Projects));
        let target = Target {
            root: "root".into(),
            epoch: "epoch".into(),
            name: "Pinned project".into(),
            entity: Entity::Project {
                id: "source".into(),
            },
        };
        let render = |app: &mut App, width, height| {
            Terminal::new(TestBackend::new(width, height))
                .unwrap()
                .draw(|f| crate::view::draw(f, app))
                .unwrap();
        };
        let key =
            |app: &mut App, code| app.input(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
        for locale in Locale::ALL {
            app.i18n = I18n::new(LocalePreference::Explicit(locale), Locale::En);
            app.apply(Action::Manage(Command::Open(target.clone(), Kind::Relink)));
            render(&mut app, 64, 22);
            assert!(
                !app.management_enabled(&Command::Save),
                "a project name cannot become a default path"
            );
            app.input(Event::Paste("/Host/目标 folder".into()));
            assert!(
                app.management_request().is_none(),
                "no API dispatch from the input stage"
            );
            assert!(key(&mut app, KeyCode::Enter).1.is_none());
            render(&mut app, 52, 22);
            let text = app
                .management
                .dialog
                .as_ref()
                .unwrap()
                .editor
                .text()
                .to_owned();
            app.input(Event::Paste("replacement".into()));
            assert_eq!(
                app.management.dialog.as_ref().unwrap().editor.text(),
                text,
                "review is immutable"
            );
            assert!(key(&mut app, KeyCode::Enter).1.is_none());
            assert!(app.management.dialog.is_none(), "review defaults to Cancel");
        }
        app.apply(Action::Manage(Command::Open(target.clone(), Kind::Relink)));
        render(&mut app, 64, 22);
        app.input(Event::Paste("/Host/original".into()));
        key(&mut app, KeyCode::Enter);
        render(&mut app, 64, 22);
        key(&mut app, KeyCode::Esc);
        assert!(!app.management.dialog.as_ref().unwrap().reviewing);
        assert_eq!(
            app.management.dialog.as_ref().unwrap().editor.text(),
            "/Host/original"
        );
        render(&mut app, 64, 22);
        key(&mut app, KeyCode::Enter);
        render(&mut app, 40, 8);
        assert!(
            app.management_request().is_none(),
            "hidden confirmation cannot dispatch"
        );
        render(&mut app, 64, 22);
        key(&mut app, KeyCode::Tab);
        key(&mut app, KeyCode::Tab);
        assert!(matches!(
            key(&mut app, KeyCode::Enter).1,
            Some(Action::Manage(Command::Save))
        ));
        let ticket = app.management_request().unwrap();
        assert_eq!(ticket.target, target);
        assert_eq!(ticket.text, "/Host/original");
        assert!(!app.management_enabled(&Command::Edit));
        app.management_completed(ticket, Err(RequestFailure::Unknown(ClientError::Timeout)));
        assert!(!app.management_enabled(&Command::Edit));
        assert!(!app.management_enabled(&Command::Save));
        key(&mut app, KeyCode::Esc);
        assert!(
            app.management.dialog.is_none(),
            "uncertain outcome can close but not become an editable retry"
        );
    }

    #[test]
    fn project_dialogs_keep_registration_scope_notes_and_uncertain_identity() {
        let mut app = App::new(
            "/unused".into(),
            I18n::new(LocalePreference::Explicit(Locale::En), Locale::En),
        );
        app.connection = ConnectionState::Connected {
            root_id: "root".into(),
            epoch: "epoch".into(),
        };
        app.apply(Action::Visit(Route::Projects));
        app.focus = crate::app::Focus::Page; // Mouse registration starts at a toolbar control.
        app.apply(app.register_project_action().unwrap());
        let mut screen = Terminal::new(TestBackend::new(64, 18)).unwrap();
        for locale in Locale::ALL {
            app.i18n = I18n::new(LocalePreference::Explicit(locale), Locale::En);
            screen
                .draw(|frame| crate::view::draw(frame, &mut app))
                .unwrap();
            assert!(
                !app.management_enabled(&Command::Save),
                "blank registration has no target"
            );
        }
        app.input(Event::Paste("/Host/中文 project".into()));
        let ticket = app.management_request().unwrap();
        assert!(matches!(ticket.target.entity, Entity::Registration));
        app.management_completed(
            ticket,
            Ok(Updated::Project(Project {
                id: "p".into(),
                aliases: vec![],
                name: "中文 project".into(),
                location_count: 1,
                archived_at: None,
                available: true,
            })),
        );
        assert_eq!(app.projects.selected.as_deref(), Some("p"));
        assert_eq!(app.focus, crate::app::Focus::List);
        assert!(app.management.dialog.is_none());
        // The next mutation targets the canonical project ID, never a session/CAS placeholder.
        let target = Target {
            root: "root".into(),
            epoch: "epoch".into(),
            name: "中文 project".into(),
            entity: Entity::Project { id: "p".into() },
        };
        assert!(!app.management_enabled(&Command::Open(target.clone(), Kind::Workspace)));
        app.apply(Action::Manage(Command::Open(target.clone(), Kind::Archive)));
        screen
            .draw(|frame| crate::view::draw(frame, &mut app))
            .unwrap();
        app.input(Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
        assert!(
            app.management.dialog.is_none(),
            "archive defaults to Cancel"
        );
        app.apply(Action::Manage(Command::Open(target, Kind::Rename)));
        screen
            .draw(|frame| crate::view::draw(frame, &mut app))
            .unwrap();
        app.input(Event::Paste("keep this edit".into()));
        let ticket = app.management_request().unwrap();
        app.management_completed(ticket, Err(RequestFailure::Unknown(ClientError::Timeout)));
        assert!(!app.management_enabled(&Command::Save));
        assert_eq!(
            app.management.dialog.as_ref().unwrap().editor.text(),
            "keep this edit"
        );
        app.abandon_management();
        assert_eq!(
            app.management.dialog.as_ref().unwrap().error,
            Some("session-edit-unknown")
        );
    }
}

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

mod consent;
mod drafts;
mod saved;
pub use saved::Checkpoint;
pub(crate) mod io;
mod view;
pub(crate) use consent::sheet as consent_sheet;
pub use io::{Output, execute};
pub use view::draw;

use crate::{
    app::{Action, App, ConnectionState, Focus},
    editor::Editor,
    navigation::Route,
};
use maka_plugins::terminal_ui::{
    Context,
    page::{Control, Page, Reply, Request as Input},
};
use maka_protocol::plugin::TerminalViewProjection;
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Open,
    Choose(usize),
    Row(usize),
    Field(usize),
    Submit(usize),
    ApproveConsent,
    Reconcile,
    Retry,
    ResumeDraft,
    ApplyDraft,
    CancelDraft,
    DraftChoice(usize, bool),
    DismissConsent,
    Back,
    Refresh,
    Discard,
    ConfirmDiscard,
    CancelDiscard,
    Next,
}
impl Command {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Open | Self::Choose(_) => "route-extensions",
            Self::Back => "extensions-back",
            Self::Refresh => "command-refresh",
            Self::Discard => "extensions-discard",
            Self::ConfirmDiscard => "extensions-forget",
            Self::CancelDiscard => "session-cancel",
            Self::Next => "sessions-next",
            Self::Row(_) => "extensions-open",
            Self::Field(_) => "extensions-edit",
            Self::Submit(_) => "extensions-save",
            Self::ApproveConsent => "extensions-authorize",
            Self::Reconcile => "extensions-reconcile",
            Self::Retry => "extensions-retry",
            Self::ResumeDraft => "extensions-resume-draft",
            Self::ApplyDraft => "extensions-continue-editing",
            Self::CancelDraft => "session-cancel",
            Self::DraftChoice(_, true) => "extensions-use-draft",
            Self::DraftChoice(_, false) => "extensions-use-current",
            Self::DismissConsent => "session-cancel",
        }
    }
}

#[derive(Clone)]
pub struct Request {
    generation: u64,
    replaying: bool,
    root: String,
    epoch: String,
    session: Option<String>,
    pub(super) work: Work,
}
impl Request {
    pub fn needs_checkpoint(&self) -> bool {
        matches!(
            self.work,
            Work::Page {
                input: Input::Submit { .. },
                ..
            } | Work::Authorize { .. }
        )
    }
}
#[derive(Clone)]
pub(super) enum Work {
    Rebind {
        view: Box<TerminalViewProjection>,
        input: Input,
    },
    Directory(Option<String>),
    Page {
        view: Box<TerminalViewProjection>,
        input: Input,
    },
    Authorize {
        view: Box<TerminalViewProjection>,
        input: Input,
        proposal: maka_plugins::authorization::Request,
    },
}
pub(super) enum Message {
    Local(&'static str),
    Remote(maka_plugins::terminal_ui::Text),
}

#[derive(Default)]
pub struct State {
    generation: u64,
    session: Option<String>,
    pub(super) directory: Vec<TerminalViewProjection>,
    next: Option<String>,
    loaded: bool,
    pub(super) view: Option<TerminalViewProjection>,
    pub(super) page: Option<Page>,
    route: Value,
    history: VecDeque<Value>,
    pub(super) drafts: BTreeMap<String, Value>,
    pub(super) editors: BTreeMap<String, Editor>,
    pending: Option<Work>,
    unresolved: Option<saved::Pending>,
    saving: bool,
    unrecorded: bool,
    confirm_discard: bool,
    review: Option<drafts::Review>,
    consent: Option<consent::Consent>,
    pub(super) busy: bool,
    writing: bool,
    pub(super) blocked: bool,
    pub(super) applied: Option<String>,
    pub(super) message: Option<Message>,
    pub(super) selected: usize,
    pub(super) top: usize,
    pub(super) reveal: bool,
    pub(super) area: Option<ratatui::layout::Rect>,
}
impl State {
    pub fn consent_visible(&self) -> bool {
        self.consent.is_some()
    }
    fn dirty_field(&self, field: &maka_plugins::terminal_ui::page::Field) -> bool {
        let value = match &field.control {
            Control::Toggle { value } => Value::Bool(*value),
            Control::Text { value, .. } => Value::String(value.clone()),
        };
        self.drafts.get(&field.id) != Some(&value)
    }
    fn dirty(&self) -> bool {
        self.page
            .as_ref()
            .is_some_and(|page| page.fields.iter().any(|field| self.dirty_field(field)))
    }
    pub fn invalidate_geometry(&mut self) {
        self.area = None;
        if let Some(consent) = &mut self.consent {
            consent.rendered = false;
        }
        for editor in self.editors.values_mut() {
            editor.invalidate_geometry();
        }
    }
    pub fn disconnect(&mut self) {
        self.saving = false;
        self.unrecorded = false;
        self.confirm_discard = false;
        self.review = None;
        self.consent = None;
        self.generation += 1;
        self.pending = None;
        self.busy = false;
        self.blocked = self.view.is_some();
        self.loaded = false;
        self.directory.clear();
        self.next = None;
        // Retain drafts, but revoke every operation on the old registration.
        self.message = self.blocked.then_some(Message::Local(
            if self.writing || self.unresolved.is_some() {
                "extensions-unknown"
            } else if self.page.is_some() {
                "extensions-restored"
            } else {
                "extensions-disconnected"
            },
        ));
        self.writing = false;
    }
    fn read(&mut self) {
        self.pending = self.view.clone().map(|view| Work::Page {
            view: Box::new(view),
            input: Input::Read {
                route: self.route.clone(),
            },
        });
        self.selected = 0;
        self.top = 0;
        self.reveal = true;
        self.area = None;
    }
    fn install(&mut self, page: Page) {
        self.drafts.clear();
        self.editors.clear();
        for field in &page.fields {
            let value = match &field.control {
                Control::Toggle { value } => Value::Bool(*value),
                Control::Text {
                    value, max_bytes, ..
                } => {
                    let mut editor = Editor::bounded(*max_bytes, "extensions-field-limit");
                    editor.insert(value);
                    editor.clear_history();
                    self.editors.insert(field.id.clone(), editor);
                    Value::String(value.clone())
                }
            };
            self.drafts.insert(field.id.clone(), value);
        }
        self.page = Some(page);
        self.selected = 0;
        self.top = 0;
        self.reveal = true;
        self.blocked = false;
    }
    fn controls(&self) -> usize {
        if let Some(review) = &self.review {
            return review.conflicts.len() * 2;
        }
        self.page.as_ref().map_or(self.directory.len(), |page| {
            page.rows.len() + page.fields.len() + page.actions.len()
        })
    }
    fn selected_command(&self) -> Option<Command> {
        if self.review.is_some() {
            return (self.selected < self.controls()).then_some(Command::DraftChoice(
                self.selected / 2,
                self.selected.is_multiple_of(2),
            ));
        }
        if let Some(page) = &self.page {
            let index = self.selected;
            if index < page.rows.len() {
                Some(Command::Row(index))
            } else if index < page.rows.len() + page.fields.len() {
                Some(Command::Field(index - page.rows.len()))
            } else {
                Some(Command::Submit(index - page.rows.len() - page.fields.len()))
            }
        } else {
            Some(Command::Choose(self.selected))
        }
    }
}

impl App {
    pub fn extensions_actions(&self) -> Vec<Action> {
        let mut commands = Vec::new();
        if self.extensions.review.is_some() {
            return [Command::CancelDraft, Command::ApplyDraft]
                .into_iter()
                .map(Action::Extension)
                .collect();
        }
        if self.extensions.confirm_discard {
            return [Command::CancelDiscard, Command::ConfirmDiscard]
                .into_iter()
                .map(Action::Extension)
                .collect();
        }
        if self
            .extensions
            .unresolved
            .as_ref()
            .is_some_and(|pending| pending.recovery.is_some())
        {
            commands.push(Command::Reconcile);
            if self.extensions.unrecorded {
                commands.push(Command::Retry);
            }
        }
        if self.extensions.view.is_some() {
            commands.push(Command::Back);
        }
        if self.extensions.blocked
            && self.extensions.page.is_some()
            && self.extensions.unresolved.is_none()
        {
            commands.push(Command::ResumeDraft);
        }
        if self.extensions.dirty() || self.extensions.blocked {
            commands.push(Command::Discard);
        } else {
            commands.push(Command::Refresh);
        }
        if self.extensions.view.is_none() && self.extensions.next.is_some() {
            commands.push(Command::Next);
        }
        commands.into_iter().map(Action::Extension).collect()
    }
    pub fn extensions_request(&mut self) -> Option<Request> {
        let ConnectionState::Connected { root_id, epoch } = &self.connection else {
            return None;
        };
        if self.extensions.busy {
            return None;
        }
        if self.navigation.current() == Route::Extensions
            && !self.extensions.loaded
            && !self.extensions.blocked
            && self.extensions.view.is_none()
            && self.extensions.pending.is_none()
        {
            // Restoring navigation is a fresh read, not restoring an old registration.
            if self.extensions.session.is_none()
                && let Route::Session(id) = self.navigation.destination(false)
                && self.tabs.contains(&id)
            {
                self.extensions.session = Some(id);
            }
            self.extensions.pending = Some(Work::Directory(None));
        }
        let work = self.extensions.pending.take()?;
        let replaying = self.extensions.unresolved.is_some();
        self.extensions.generation += 1;
        self.extensions.busy = true;
        self.extensions.writing = matches!(
            work,
            Work::Page {
                input: Input::Submit { .. },
                ..
            } | Work::Authorize { .. }
        );
        if self.extensions.writing {
            let (input, proposal) = match &work {
                Work::Page { input, .. } => (input.clone(), None),
                Work::Authorize {
                    input, proposal, ..
                } => (input.clone(), Some(proposal.clone())),
                _ => unreachable!(),
            };
            let recovery = match &input {
                Input::Submit { action, .. } => self
                    .extensions
                    .page
                    .as_ref()
                    .and_then(|page| page.actions.iter().find(|item| &item.id == action))
                    .and_then(|action| action.recovery.clone()),
                _ => None,
            };
            self.extensions.unresolved = Some(saved::Pending {
                input,
                proposal,
                recovery,
            });
            self.extensions.saving = true;
            self.extensions.unrecorded = false;
        }
        self.extensions.message = None;
        Some(Request {
            generation: self.extensions.generation,
            replaying,
            root: root_id.clone(),
            epoch: epoch.clone(),
            session: self.extensions.session.clone(),
            work,
        })
    }
    pub fn extensions_complete(&mut self, request: Request, result: Result<Output, io::Failure>) {
        if request.generation != self.extensions.generation
            || !matches!(&self.connection,
            ConnectionState::Connected { root_id, epoch } if *root_id == request.root && *epoch == request.epoch)
        {
            return;
        }
        let state = &mut self.extensions;
        state.busy = false;
        state.saving = false;
        state.writing = false;
        state.area = None;
        let recovering = matches!(
            request.work,
            Work::Rebind {
                input: Input::Recover { .. },
                ..
            }
        );
        let reloading = matches!(
            request.work,
            Work::Rebind {
                input: Input::Read { .. },
                ..
            }
        );
        let result = match result {
            Ok(Output::Rebound {
                view,
                reply: Reply::Page { page },
            }) if reloading => {
                state.reload_draft(*view, page);
                self.hits.clear();
                self.hover = None;
                return;
            }
            Ok(Output::Rebound { view, reply }) if recovering || reloading => {
                state.view = Some(*view);
                Ok(Output::Page(reply))
            }
            Ok(Output::Rebound { .. }) => Err(io::Failure { unknown: false }),
            result => result,
        };
        // A rejected recovery read says nothing about the original write.
        if !recovering
            && !request.replaying
            && request.needs_checkpoint()
            && matches!(
                &result,
                Ok(Output::Page(
                    Reply::Consent { .. } | Reply::Conflict | Reply::Rejected { .. }
                )) | Err(io::Failure { unknown: false })
            )
        {
            state.unresolved = None;
        }
        match result {
            Ok(Output::Rebound { .. }) => unreachable!(),
            Ok(Output::Page(Reply::Unrecorded)) => {
                state.blocked = true;
                state.unrecorded = recovering;
                state.message = Some(Message::Local("extensions-unrecorded"));
            }
            Ok(Output::Directory(page)) => {
                state.loaded = true;
                state.directory = page.items;
                state.next = page.next_cursor;
                state.page = None;
                state.view = None;
                state.blocked = false;
                state.selected = 0;
                state.top = 0;
                state.reveal = true;
            }
            Ok(Output::Page(Reply::Page { page })) => state.install(page),
            Ok(Output::Page(Reply::Consent { request: proposal })) => {
                // A retry needing consent does not settle an earlier lost write.
                state.blocked = request.replaying;
                // Preparing consent does not authorize anything. Leaving the page
                // cancels presentation, without discarding the form or opening a
                // delayed modal over another destination.
                if self.navigation.current() != Route::Extensions {
                    return;
                }
                if let Work::Page {
                    view,
                    input: input @ Input::Submit { grant: None, .. },
                } = request.work
                {
                    state.consent = Some(consent::Consent {
                        view,
                        input,
                        proposal,
                        rendered: false,
                    });
                } else {
                    state.blocked = true;
                    state.message = Some(Message::Local("extensions-failed"));
                }
            }
            Ok(Output::Page(Reply::Applied { route })) => {
                state.unresolved = None;
                state.unrecorded = false;
                state.blocked = false;
                state.applied = match &request.work {
                    Work::Page {
                        input:
                            Input::Submit {
                                route: source,
                                action,
                                ..
                            },
                        ..
                    } if source == &route => Some(action.clone()),
                    _ => None,
                };
                if state.route != route {
                    state.history.clear();
                }
                state.route = route;
                state.page = None;
                state.drafts.clear();
                state.editors.clear();
                state.read();
            }
            Ok(Output::Page(Reply::Conflict)) => {
                state.blocked = true;
                state.message = Some(Message::Local("extensions-conflict"));
            }
            Ok(Output::Page(Reply::Rejected { message })) => {
                state.blocked = recovering || reloading || request.replaying;
                state.message = Some(Message::Remote(message));
            }
            Err(failure) => {
                state.blocked = true;
                state.message = Some(Message::Local(if failure.unknown {
                    "extensions-unknown"
                } else {
                    "extensions-failed"
                }));
            }
        }
        self.hits.clear();
        self.hover = None;
    }
    /// Approval needs the terms on screen; the sheet layer reports that.
    pub(crate) fn consent_presented(&mut self, shown: bool) {
        if let Some(consent) = &mut self.extensions.consent {
            consent.rendered = shown;
        }
    }
    pub fn extensions_enabled(&self, command: &Command) -> bool {
        let state = &self.extensions;
        if let Some(consent) = &state.consent {
            return match command {
                Command::DismissConsent => true,
                Command::ApproveConsent => {
                    consent.rendered
                        && !state.busy
                        && (!state.blocked || state.unresolved.is_some())
                        && self.navigation.current() == Route::Extensions
                        && matches!(self.connection, ConnectionState::Connected { .. })
                }
                _ => false,
            };
        }
        if *command == Command::Open {
            return matches!(self.connection, ConnectionState::Connected { .. }) && !state.busy;
        }
        if !matches!(self.connection, ConnectionState::Connected { .. })
            || self.navigation.current() != Route::Extensions
            || state.busy
            || state.pending.is_some()
        {
            return false;
        }
        match command {
            Command::ResumeDraft => {
                state.blocked
                    && state.page.is_some()
                    && state.unresolved.is_none()
                    && state.review.is_none()
            }
            Command::ApplyDraft => {
                state
                    .review
                    .as_ref()
                    .is_some_and(|review| review.resolved())
                    && state.unresolved.is_none()
            }
            Command::CancelDraft => state.review.is_some(),
            Command::DraftChoice(index, _) => state
                .review
                .as_ref()
                .is_some_and(|review| *index < review.conflicts.len()),
            Command::Reconcile => {
                !state.confirm_discard
                    && state
                        .unresolved
                        .as_ref()
                        .is_some_and(|pending| pending.recovery.is_some())
            }
            Command::Retry => {
                !state.confirm_discard
                    && state.unrecorded
                    && state
                        .unresolved
                        .as_ref()
                        .is_some_and(|pending| pending.recovery.is_some())
            }
            Command::ConfirmDiscard | Command::CancelDiscard => state.confirm_discard,
            Command::Discard => state.dirty() || state.blocked,
            Command::Back => !state.dirty() && state.unresolved.is_none() && !state.blocked,
            Command::Refresh => !state.dirty() && !state.blocked,
            Command::Next => state.view.is_none() && state.next.is_some() && !state.blocked,
            Command::Choose(index) => {
                !state.blocked
                    && state.directory.get(*index).is_some_and(|view| {
                        view.descriptor.context == Context::Application || state.session.is_some()
                    })
            }
            Command::Row(index) => {
                !state.blocked
                    && !state.dirty()
                    && state
                        .page
                        .as_ref()
                        .is_some_and(|page| *index < page.rows.len())
            }
            Command::Field(index) => {
                !state.blocked
                    && state
                        .page
                        .as_ref()
                        .and_then(|page| page.fields.get(*index))
                        .is_some_and(|field| field.enabled)
            }
            Command::Submit(index) => {
                !state.blocked
                    && state.page.as_ref().is_some_and(|page| {
                        page.actions.get(*index).is_some_and(|action| {
                            action.enabled
                                && page.fields.iter().all(|field| {
                                    !state.dirty_field(field) || action.fields.contains(&field.id)
                                })
                        })
                    })
            }
            Command::Open => false,
            Command::ApproveConsent | Command::DismissConsent => false,
        }
    }
    pub fn extensions_action(&mut self, command: Command) {
        if !self.extensions_enabled(&command) {
            return;
        }
        if command == Command::Open {
            if self.extensions.dirty()
                || self.extensions.blocked
                || self.extensions.unresolved.is_some()
            {
                self.apply(Action::Visit(Route::Extensions));
                return;
            }
            let session = match self.navigation.current() {
                Route::Session(id) => Some(id),
                _ => None,
            };
            let generation = self.extensions.generation + 1;
            self.extensions = State {
                session,
                generation,
                pending: Some(Work::Directory(None)),
                ..State::default()
            };
            self.apply(Action::Visit(Route::Extensions));
            return;
        }
        self.focus = Focus::List;
        let state = &mut self.extensions;
        state.applied = None;
        match command {
            Command::ResumeDraft => {
                state.pending = Some(Work::Rebind {
                    view: Box::new(state.view.clone().unwrap()),
                    input: Input::Read {
                        route: state.route.clone(),
                    },
                });
            }
            Command::ApplyDraft => state.accept_draft(),
            Command::CancelDraft => {
                state.review = None;
                state.selected = 0;
                state.top = 0;
                state.message = Some(Message::Local("extensions-restored"));
            }
            Command::DraftChoice(index, mine) => {
                state.review.as_mut().unwrap().conflicts[index].mine = Some(mine);
                state.selected = index * 2 + usize::from(!mine);
            }
            Command::Reconcile => {
                state.unrecorded = false;
                state.pending = Some(Work::Rebind {
                    view: Box::new(state.view.clone().unwrap()),
                    input: Input::Recover {
                        route: state.unresolved.as_ref().unwrap().recovery.clone().unwrap(),
                    },
                });
            }
            Command::Retry => {
                state.unrecorded = false;
                let pending = state.unresolved.as_ref().unwrap();
                let view = Box::new(state.view.clone().unwrap());
                state.pending = Some(match &pending.proposal {
                    Some(proposal) => Work::Authorize {
                        view,
                        input: pending.input.clone(),
                        proposal: proposal.clone(),
                    },
                    None => Work::Page {
                        view,
                        input: pending.input.clone(),
                    },
                });
            }
            Command::Discard if state.unresolved.is_some() => {
                state.confirm_discard = true;
                state.message = Some(Message::Local("extensions-forget-confirm"));
            }
            Command::CancelDiscard => {
                state.confirm_discard = false;
                state.message = Some(Message::Local("extensions-unknown"));
            }
            Command::ApproveConsent => {
                let consent = state.consent.take().unwrap();
                state.pending = Some(Work::Authorize {
                    view: consent.view,
                    input: consent.input,
                    proposal: consent.proposal,
                });
            }
            Command::DismissConsent => {
                state.consent = None;
            }
            Command::Choose(index) => {
                state.view = Some(state.directory[index].clone());
                state.route = Value::Null;
                state.history.clear();
                state.read();
            }
            Command::Row(index) => {
                let route = state.page.as_ref().unwrap().rows[index].route.clone();
                if state.history.len() == 64 {
                    state.history.pop_front();
                }
                state.history.push_back(state.route.clone());
                state.route = route;
                state.read();
            }
            Command::Field(index) => {
                let page = state.page.as_ref().unwrap();
                let field = &page.fields[index];
                state.selected = page.rows.len() + index;
                if let Some(Value::Bool(value)) = state.drafts.get_mut(&field.id) {
                    *value = !*value;
                }
            }
            Command::Submit(index) => {
                let page = state.page.as_ref().unwrap();
                let action = &page.actions[index];
                let fields = action
                    .fields
                    .iter()
                    .filter_map(|id| {
                        state
                            .drafts
                            .get(id)
                            .map(|value| (id.clone(), value.clone()))
                    })
                    .collect();
                match page.submission(state.route.clone(), &action.id, fields) {
                    Ok(input) => {
                        state.pending = state.view.clone().map(|view| Work::Page {
                            view: Box::new(view),
                            input,
                        })
                    }
                    Err(_) => state.message = Some(Message::Local("extensions-invalid-fields")),
                }
            }
            Command::Back => {
                if let Some(route) = state.history.pop_back() {
                    state.route = route;
                    state.read();
                } else {
                    state.page = None;
                    state.view = None;
                    state.directory.clear();
                    state.next = None;
                    state.loaded = false;
                    state.drafts.clear();
                    state.editors.clear();
                    state.pending = Some(Work::Directory(None));
                }
            }
            Command::Discard | Command::ConfirmDiscard => {
                state.review = None;
                state.unresolved = None;
                state.unrecorded = false;
                state.confirm_discard = false;
                state.message = None;
                state.generation += 1;
                state.page = None;
                state.view = None;
                state.directory.clear();
                state.next = None;
                state.loaded = false;
                state.drafts.clear();
                state.editors.clear();
                state.blocked = false;
                state.history.clear();
                state.pending = Some(Work::Directory(None));
            }
            Command::Refresh => {
                if state.view.is_some() {
                    state.read();
                } else {
                    state.pending = Some(Work::Directory(None));
                }
            }
            Command::Next => state.pending = Some(Work::Directory(state.next.clone())),
            Command::Open => {}
        }
        state.area = None;
        self.hits.clear();
        self.hover = None;
    }
    pub fn extensions_input(&mut self, event: &crossterm::event::Event) -> bool {
        use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
        if self.palette.is_some() || self.navigation.current() != Route::Extensions {
            return false;
        }
        if self.extensions.review.is_some()
            && matches!(event, Event::Key(key) if key.kind != KeyEventKind::Release && key.code == KeyCode::Esc)
        {
            self.extensions_action(Command::CancelDraft);
            return true;
        }
        if let Event::Mouse(mouse) = event
            && self
                .extensions
                .area
                .is_some_and(|area| area.contains((mouse.column, mouse.row).into()))
            && matches!(
                mouse.kind,
                MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
            )
        {
            self.extensions.top = if mouse.kind == MouseEventKind::ScrollDown {
                self.extensions.top.saturating_add(3)
            } else {
                self.extensions.top.saturating_sub(3)
            };
            return true;
        }
        if let Event::Mouse(mouse) = event
            && !self.extensions.busy
            && !self.extensions.blocked
            && self.extensions.pending.is_none()
            && let Some(field_id) = self.extensions.page.as_ref().and_then(|page| {
                page.fields
                    .iter()
                    .find(|field| {
                        field.enabled
                            && self
                                .extensions
                                .editors
                                .get(&field.id)
                                .is_some_and(|editor| {
                                    editor.contains((mouse.column, mouse.row).into())
                                        || editor.dragging()
                                })
                    })
                    .map(|field| field.id.clone())
            })
            && self
                .extensions
                .editors
                .get_mut(&field_id)
                .unwrap()
                .mouse(*mouse)
        {
            self.focus = Focus::List;
            let page = self.extensions.page.as_ref().unwrap();
            self.extensions.selected = page.rows.len()
                + page
                    .fields
                    .iter()
                    .position(|field| field.id == field_id)
                    .unwrap();
            return true;
        }
        if self.focus != Focus::List || self.extensions.area.is_none() {
            return false;
        }
        let multiline = matches!(self.extensions.selected_command(), Some(Command::Field(index))
            if self.extensions.page.as_ref().is_some_and(|page| page.fields[index].enabled
                && matches!(page.fields[index].control, Control::Text { multiline: true, .. })));
        if let Event::Key(key) = event
            && key.kind != KeyEventKind::Release
            && !(multiline && matches!(key.code, KeyCode::Enter | KeyCode::Up | KeyCode::Down))
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            match key.code {
                KeyCode::Tab | KeyCode::Down | KeyCode::BackTab | KeyCode::Up => {
                    let n = self.extensions.controls();
                    let backwards =
                        key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT);
                    if matches!(key.code, KeyCode::Tab | KeyCode::BackTab)
                        && (n == 0
                            || backwards && self.extensions.selected == 0
                            || !backwards && self.extensions.selected + 1 >= n)
                    {
                        return false; // Continue through the page toolbar and navigation.
                    }
                    self.extensions.reveal = true;
                    if n > 0 {
                        self.extensions.selected =
                            if matches!(key.code, KeyCode::Up | KeyCode::BackTab) {
                                (self.extensions.selected + n - 1) % n
                            } else {
                                (self.extensions.selected + 1) % n
                            };
                    }
                    return true;
                }
                KeyCode::PageDown | KeyCode::PageUp => {
                    let height = self.extensions.area.unwrap().height as usize;
                    self.extensions.top = if key.code == KeyCode::PageDown {
                        self.extensions.top.saturating_add(height)
                    } else {
                        self.extensions.top.saturating_sub(height)
                    };
                    return true;
                }
                KeyCode::Char(' ') if matches!(self.extensions.selected_command(), Some(Command::Field(index)) if self.extensions.page.as_ref().is_some_and(|page| matches!(page.fields[index].control, Control::Toggle { .. }))) =>
                {
                    self.extensions_action(self.extensions.selected_command().unwrap());
                    return true;
                }
                KeyCode::Esc => {
                    self.extensions_action(if self.extensions.review.is_some() {
                        Command::CancelDraft
                    } else {
                        Command::Back
                    });
                    return true;
                }
                KeyCode::Enter => {
                    if let Some(command) = self.extensions.selected_command() {
                        self.extensions_action(command);
                    }
                    return true;
                }
                _ => {}
            }
        }
        let state = &mut self.extensions;
        if state.busy || state.pending.is_some() || state.blocked {
            return false;
        }
        let Some(page) = &state.page else {
            return false;
        };
        let Some(index) = state.selected.checked_sub(page.rows.len()) else {
            return false;
        };
        let Some(field) = page.fields.get(index).filter(|field| field.enabled) else {
            return false;
        };
        let Some(editor) = state.editors.get_mut(&field.id) else {
            return false;
        };
        let changed = match event {
            Event::Key(key)
                if key.kind != KeyEventKind::Release
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && !matches!(
                        key.code,
                        KeyCode::Char('a' | 'z' | 'y' | 'u' | 'k')
                            | KeyCode::Left
                            | KeyCode::Right
                            | KeyCode::Backspace
                            | KeyCode::Delete
                    )
                {
                    return false;
                }
                editor.key(*key)
            }
            Event::Paste(text) => {
                if matches!(
                    field.control,
                    Control::Text {
                        multiline: false,
                        ..
                    }
                ) {
                    editor.insert(&crate::view::safe(text))
                } else {
                    editor.insert(text)
                }
            }
            _ => return false,
        };
        if changed {
            state.applied = None;
            state
                .drafts
                .insert(field.id.clone(), Value::String(editor.text().into()));
        }
        true
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::i18n::{I18n, LocalePreference};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use maka_plugins::{
        composition::Scope,
        remote::Target,
        terminal_ui::{
            Descriptor, Text, VERSION,
            page::{Action as PageAction, Field},
        },
    };
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;

    fn projection() -> TerminalViewProjection {
        TerminalViewProjection {
            package_id: "example.notes".into(),
            scope_id: Scope::Profile,
            method: "preferences".into(),
            target: Target {
                entry_id: "notes".into(),
                activation: uuid::Uuid::new_v4().to_string(),
                registration: uuid::Uuid::new_v4(),
            },
            descriptor: Descriptor {
                version: VERSION,
                title: Text::localized("Notes", "笔记", "筆記"),
                context: Context::Session,
            },
        }
    }
    fn form() -> Page {
        Page {
            version: VERSION,
            title: Text::plain("Notebook"),
            revision: "one".into(),
            body: "A plugin-owned form.".into(),
            rows: vec![],
            fields: vec![
                Field {
                    id: "enabled".into(),
                    label: Text::plain("Enabled"),
                    enabled: true,
                    control: Control::Toggle { value: true },
                },
                Field {
                    id: "name".into(),
                    label: Text::plain("Name"),
                    enabled: true,
                    control: Control::Text {
                        value: "My notes".into(),
                        max_bytes: 128,
                        multiline: false,
                    },
                },
            ],
            actions: vec![PageAction {
                id: "save".into(),
                label: Text::plain("Save"),
                enabled: true,
                fields: vec!["enabled".into(), "name".into()],
                recovery: None,
            }],
        }
    }
    fn draw(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| crate::view::draw(frame, app))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }
    pub(crate) fn app() -> App {
        let mut app = App::new(
            "/test".into(),
            I18n::new(
                LocalePreference::Explicit(crate::i18n::Locale::En),
                crate::i18n::Locale::En,
            ),
        );
        app.connection = ConnectionState::Connected {
            root_id: "root".into(),
            epoch: "epoch".into(),
        };
        app.apply(Action::Visit(Route::Session("session".into())));
        app.extensions_action(Command::Open);
        let request = app.extensions_request().unwrap();
        assert_eq!(request.session.as_deref(), Some("session"));
        app.extensions_complete(
            request,
            Ok(Output::Directory(maka_protocol::plugin::Page {
                items: vec![projection()],
                next_cursor: None,
            })),
        );
        app.extensions_action(Command::Choose(0));
        let request = app.extensions_request().unwrap();
        app.extensions_complete(request, Ok(Output::Page(Reply::Page { page: form() })));
        app
    }
    #[test]
    fn contributed_form_supports_mouse_keyboard_and_retains_conflicting_drafts_without_rebinding() {
        let mut app = app();
        assert!(draw(&mut app, 90, 26).contains("Notebook"));
        let hit = app
            .hits
            .iter()
            .find(|hit| hit.action == Action::Extension(Command::Field(0)))
            .unwrap()
            .area;
        let event = Event::Mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: hit.x + hit.width / 2,
            row: hit.y,
            modifiers: KeyModifiers::NONE,
        });
        app.input(event);
        assert_eq!(app.extensions.drafts["enabled"], json!(false));
        draw(&mut app, 90, 26);
        assert!(!app.extensions_enabled(&Command::Refresh));
        app.input(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
        app.input(Event::Key(KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::CONTROL,
        )));
        app.input(Event::Paste("Renamed".into()));
        assert_eq!(app.extensions.drafts["name"], json!("Renamed"));
        app.extensions.page.as_mut().unwrap().actions[0]
            .fields
            .pop();
        assert!(
            !app.extensions_enabled(&Command::Submit(0)),
            "saving a subset must not discard other drafts"
        );
        app.extensions.page.as_mut().unwrap().actions[0]
            .fields
            .push("name".into());
        app.extensions_action(Command::Submit(0));
        let request = app.extensions_request().unwrap();
        let Work::Page {
            view,
            input: Input::Submit {
                fields, revision, ..
            },
        } = &request.work
        else {
            panic!("submit");
        };
        assert_eq!(view.target, app.extensions.view.as_ref().unwrap().target);
        assert_eq!(fields["enabled"], json!(false));
        assert_eq!(fields["name"], json!("Renamed"));
        assert_eq!(revision, "one");
        app.extensions_complete(request.clone(), Ok(Output::Page(Reply::Conflict)));
        assert_eq!(app.extensions.drafts["name"], json!("Renamed"));
        assert!(!app.extensions_enabled(&Command::Submit(0)));
        assert!(
            app.extensions_request().is_none(),
            "no automatic refresh or retry"
        );
        assert!(draw(&mut app, 44, 18).contains("draft"));
        app.extensions_action(Command::Discard);
        let fresh = app.extensions_request().unwrap();
        app.extensions_complete(request, Ok(Output::Page(Reply::Page { page: form() })));
        assert!(
            app.extensions.page.is_none(),
            "late old result cannot replace new directory"
        );
        app.extensions_complete(
            fresh,
            Ok(Output::Directory(maka_protocol::plugin::Page {
                items: vec![projection()],
                next_cursor: None,
            })),
        );
        assert_eq!(app.extensions.directory.len(), 1);
    }

    #[test]
    fn consent_requires_visible_explicit_confirmation_and_never_steals_another_page() {
        use maka_plugins::authorization::{Capability, Request as Proposal, Target};
        let proposal = Proposal {
            operation_id: uuid::Uuid::new_v4(),
            title: "Untrusted title".into(),
            target: Target::Profile,
            capabilities: [Capability::Notifications].into(),
        };
        let mut app = app();
        app.extensions.drafts.insert("name".into(), json!("Draft"));
        let editor = app.extensions.editors.get_mut("name").unwrap();
        editor.clear_if_unchanged("My notes");
        editor.insert("Draft");
        let prepare = |app: &mut App| {
            app.extensions_action(Command::Submit(0));
            let request = app.extensions_request().unwrap();
            app.extensions_complete(
                request,
                Ok(Output::Page(Reply::Consent {
                    request: proposal.clone(),
                })),
            );
        };
        prepare(&mut app);
        assert!(app.extensions_request().is_none());
        assert!(!app.extensions_enabled(&Command::ApproveConsent));
        let screen = draw(&mut app, 90, 28);
        assert!(screen.contains("Send notifications"));
        assert!(!screen.contains("Untrusted title"));
        app.input(Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
        assert!(
            !app.extensions.consent_visible(),
            "Enter defaults to cancel"
        );
        assert_eq!(app.extensions.drafts["name"], json!("Draft"));
        assert!(app.extensions_request().is_none());

        prepare(&mut app);
        draw(&mut app, 90, 28);
        app.input(Event::Mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }));
        assert!(!app.extensions.consent_visible());
        assert_eq!(app.navigation.current(), Route::Extensions);
        assert!(app.extensions_request().is_none());

        prepare(&mut app);
        draw(&mut app, 30, 10);
        app.extensions_action(Command::ApproveConsent);
        assert!(
            app.extensions.consent_visible(),
            "hidden terms cannot be approved"
        );
        draw(&mut app, 90, 28);
        app.input(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));
        app.input(Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        )));
        let request = app.extensions_request().unwrap();
        let Work::Authorize {
            view,
            input,
            proposal: actual,
        } = &request.work
        else {
            panic!("explicit approval")
        };
        assert_eq!(view.target, app.extensions.view.as_ref().unwrap().target);
        assert_eq!(actual, &proposal);
        let Input::Submit { fields, grant, .. } = input else {
            panic!("submit")
        };
        assert_eq!(fields["name"], json!("Draft"));
        assert!(grant.is_none());
        let saved = app.extensions.checkpoint("root").unwrap();
        saved.validate("root").unwrap();
        let mut restored = State::default();
        restored.restore(saved).unwrap();
        assert!(restored.pending.is_none());
        assert!(
            restored.consent.is_none(),
            "restoring explicit consent is not another approval"
        );
        assert_eq!(
            restored.unresolved.unwrap().proposal.as_ref(),
            Some(&proposal)
        );
        app.extensions_complete(request, Err(io::Failure { unknown: true }));
        assert!(!app.extensions_enabled(&Command::Submit(0)));
        assert!(app.extensions_request().is_none());

        let mut app = self::app();
        app.extensions_action(Command::Submit(0));
        let request = app.extensions_request().unwrap();
        app.apply(Action::Visit(Route::Workspace));
        app.extensions_complete(
            request,
            Ok(Output::Page(Reply::Consent { request: proposal })),
        );
        assert!(!app.extensions.consent_visible());
        assert!(app.extensions_request().is_none());
    }

    #[test]
    fn disconnect_revokes_controls_preserves_drafts_and_tiny_layout_has_no_stale_clicks() {
        let mut app = app();
        draw(&mut app, 80, 24);
        app.extensions_action(Command::Field(0));
        app.extensions_action(Command::Submit(0));
        let request = app.extensions_request().unwrap();
        app.extensions.disconnect();
        app.extensions_complete(
            request,
            Ok(Output::Page(Reply::Applied { route: Value::Null })),
        );
        assert_eq!(app.extensions.drafts["enabled"], json!(false));
        assert!(matches!(
            app.extensions.message,
            Some(Message::Local("extensions-unknown"))
        ));
        assert!(!app.extensions_enabled(&Command::Submit(0)));
        draw(&mut app, 25, 8);
        assert!(app.extensions.area.is_none());
        assert!(
            !app.hits
                .iter()
                .any(|hit| matches!(hit.action, Action::Extension(_)))
        );
        assert!(app.extensions_request().is_none());
    }

    #[test]
    fn saved_unknown_submission_queries_first_and_retries_only_the_original_idempotent_intent() {
        let mut app = app();
        let recovery = json!({"operation":"one"});
        app.extensions.page.as_mut().unwrap().actions[0].recovery = Some(recovery.clone());
        app.extensions_action(Command::Field(0));
        app.extensions_action(Command::Submit(0));
        let original = app.extensions_request().unwrap();
        assert!(original.needs_checkpoint());
        let frozen = app.extensions.unresolved.as_ref().unwrap().input.clone();
        let saved = serde_json::to_value(app.extensions.checkpoint("root").unwrap()).unwrap();
        for (pointer, value) in [
            ("/root", json!("other")),
            ("/pending/input/revision", json!("different")),
            ("/pending/input/fields/enabled", json!(true)),
            ("/pending/recovery", json!({"operation":"other"})),
            ("/cursors/name/cursor", json!(999)),
            ("/drafts/name", json!("x".repeat(129))),
            ("/view/target/activation", json!("not-an-activation")),
        ] {
            let mut invalid = saved.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            assert!(
                serde_json::from_value::<Checkpoint>(invalid)
                    .unwrap()
                    .validate("root")
                    .is_err(),
                "{pointer}"
            );
        }
        app.extensions.disconnect();
        assert!(!app.extensions_after_checkpoint(&original, &Ok(())));
        app.extensions = State::default();
        app.extensions
            .restore(serde_json::from_value(saved).unwrap())
            .unwrap();
        assert!(app.extensions_request().is_none());
        assert!(!app.extensions_enabled(&Command::Retry));
        assert!(!app.extensions_enabled(&Command::Back));
        assert!(!app.extensions_enabled(&Command::Submit(0)));
        app.apply(Action::Visit(Route::Workspace));
        app.extensions_action(Command::Open);
        assert_eq!(app.navigation.current(), Route::Extensions);
        app.extensions_action(Command::Reconcile);
        let query = app.extensions_request().unwrap();
        assert!(
            matches!(&query.work, Work::Rebind { input: Input::Recover { route }, .. } if route == &recovery)
        );
        assert!(!query.needs_checkpoint());
        app.extensions_complete(query, Err(io::Failure { unknown: false }));
        assert!(app.extensions.unresolved.is_some());
        assert!(!app.extensions_enabled(&Command::Retry));
        app.extensions_action(Command::Reconcile);
        let query = app.extensions_request().unwrap();
        let mut view = app.extensions.view.clone().unwrap();
        view.target.registration = uuid::Uuid::new_v4();
        app.extensions_complete(
            query,
            Ok(Output::Rebound {
                view: Box::new(view.clone()),
                reply: Reply::Unrecorded,
            }),
        );
        assert!(draw(&mut app, 58, 24).contains("Retry original submission"));
        app.extensions_action(Command::Retry);
        let retry = app.extensions_request().unwrap();
        assert!(retry.needs_checkpoint());
        assert!(
            matches!(&retry.work, Work::Page { view: actual, input } if actual.target == view.target && input == &frozen)
        );
        assert!(!app.extensions_after_checkpoint(&original, &Ok(())));
        assert!(app.extensions_after_checkpoint(&retry, &Ok(())));
        assert!(!app.extensions_after_checkpoint(&retry, &Ok(())));
        // A rejection of this retry cannot settle the earlier uncertain attempt.
        app.extensions_complete(retry, Err(io::Failure { unknown: false }));
        assert!(app.extensions.unresolved.is_some());
        assert!(!app.extensions_enabled(&Command::Submit(0)));
        app.extensions_action(Command::Discard);
        assert!(app.extensions.unresolved.is_some());
        assert!(app.extensions_request().is_none());
        assert!(draw(&mut app, 90, 24).contains("does not cancel"));
        app.extensions_action(Command::CancelDiscard);
        app.extensions_action(Command::Reconcile);
        let query = app.extensions_request().unwrap();
        app.extensions_complete(
            query,
            Ok(Output::Rebound {
                view: Box::new(view),
                reply: Reply::Applied {
                    route: json!({"task":"original"}),
                },
            }),
        );
        assert!(app.extensions.unresolved.is_none());
        assert!(
            matches!(app.extensions_request().unwrap().work, Work::Page { input: Input::Read { route }, .. } if route == json!({"task":"original"}))
        );
    }
}

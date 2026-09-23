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

//! Native terminal client. Host business logic remains outside this crate.
mod app;
mod chrome;
mod editor;
mod files;
mod i18n;
mod motion;
mod navigation;
mod pages;
mod state;
mod terminal;
mod theme;
mod view;

use app::{Action, App, ConnectionState, Notice};
use crossterm::event::EventStream;
use futures_util::StreamExt;
pub use i18n::{Locale, LocalePreference};
use maka_client::{Client, Error, Notification};
use maka_protocol::Operation;
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use tokio::{sync::mpsc, task::JoinSet};

pub struct Options {
    pub root: PathBuf,
    pub locale: Option<LocalePreference>,
    pub profile: String,
}

enum Completed {
    Revised(
        pages::revision::Request,
        Result<pages::revision::Output, maka_client::RequestFailure>,
    ),
    Branched(
        pages::branch::Request,
        Result<pages::branch::Output, maka_client::RequestFailure>,
    ),
    Removal(
        pages::manage::removal::Request,
        Result<pages::manage::removal::Output, maka_client::RequestFailure>,
    ),
    Oauth(
        pages::manage::oauth::Request,
        Result<pages::manage::oauth::Output, maka_client::RequestFailure>,
    ),
    Managed(
        Box<pages::manage::Ticket>,
        Result<pages::manage::Updated, maka_client::RequestFailure>,
    ),
    History(
        pages::chat::history::Request,
        Result<pages::chat::history::Output, String>,
    ),
    Queue(
        pages::queue::Ticket,
        Result<maka_protocol::message::MutationResult, maka_client::RequestFailure>,
    ),
    Stopped(
        pages::chat::stopping::Target,
        Result<maka_protocol::turn::TurnSnapshot, maka_client::RequestFailure>,
    ),
    Context(
        pages::chat::context::Request,
        Result<maka_protocol::context::ContextDiagnosticsResult, String>,
    ),
    Interaction(
        Box<(
            pages::interactions::Ticket,
            Result<maka_protocol::interaction::InteractionSnapshot, maka_client::RequestFailure>,
        )>,
    ),
    Reconciled(
        pages::sending::Submission,
        Result<Option<maka_protocol::message::ExecutionResolution>, maka_client::RequestFailure>,
    ),
    Created(
        navigation::Route,
        Result<Box<maka_protocol::session::SessionCatalogProjection>, String>,
    ),
    Submitted(
        pages::sending::Submission,
        Result<maka_protocol::message::SubmitResult, maka_client::RequestFailure>,
    ),
    ChatOpened(
        pages::chat::OpenRequest,
        Result<Box<pages::chat::Opened>, String>,
    ),
    ChatPage(
        pages::chat::PageRequest,
        Result<maka_client::transcript::TranscriptBatch, String>,
    ),
    ChatReady(String, Result<(), String>),
    ObservationClosed,
    Connected(Result<(Client, mpsc::Receiver<Notification>), Error>),
    Status(Result<Value, String>),
    Catalog(Result<maka_protocol::session::SessionCatalogQueryResult, String>),
    Inbox(Result<maka_protocol::session::SessionCatalogQueryResult, String>),
    Projects(Result<maka_protocol::project::QueryResult, String>),
    Connections(Result<Value, String>),
    Directory(
        pages::manage::directory::Request,
        Result<maka_protocol::project::QueryResult, String>,
    ),
    ChooseProject(
        pages::manage::choose_project::Request,
        Result<maka_protocol::project::QueryResult, String>,
    ),
    Locations(
        pages::manage::locations::Request,
        Result<maka_protocol::project::QueryResult, String>,
    ),
    Session(
        pages::sessions::DetailRequest,
        Result<Option<Box<maka_protocol::session::SessionCatalogProjection>>, String>,
    ),
    Models(pages::manage::models::Request, Result<Value, String>),
    EnabledModels(
        pages::manage::enabled_models::Request,
        Result<Value, String>,
    ),
    Credential(
        pages::manage::credentials::Request,
        Result<
            maka_protocol::configuration::CredentialVaultQueryResult,
            maka_client::RequestFailure,
        >,
    ),
    Onboard(
        pages::onboarding::Ticket,
        Result<pages::onboarding::ResultValue, maka_client::RequestFailure>,
    ),
}

pub async fn run(options: Options) -> Result<(), Error> {
    let i18n = i18n::I18n::from_environment(options.locale)?;
    let (_guard, mut screen) = terminal::Guard::enter(&i18n)?;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        terminal::restore();
        previous(info);
    }));
    let mut app = App::new(options.root, i18n);
    app.theme = theme::Theme::from_environment();
    let keep_locale = options.locale.is_some() || std::env::var_os("MAKA_LOCALE").is_some();
    let mut state = match state::State::open(&app.root, &options.profile).await {
        Ok(Some((state, saved))) => {
            if let Some(saved) = saved {
                saved.restore(&mut app, keep_locale)?;
            }
            Some(state)
        }
        Ok(None) => None,
        Err(error) => {
            return Err(app
                .i18n
                .format("state-open-failed", &[("error", &error.to_string())])
                .into());
        }
    };
    let mut input = EventStream::new();
    let mut jobs = JoinSet::new();
    // Local configuration I/O is not scoped to a Host connection epoch.
    let mut theme_jobs = JoinSet::new();
    let mut history_job = None;
    let mut client: Option<Client> = None;
    let mut notifications: Option<mpsc::Receiver<Notification>> = None;
    let mut oauth_service: Option<maka_client::OAuthPresentationService> = None;
    let mut effect = app.apply(Action::Connect);
    let mut dirty = true;
    let mut flushed = false;
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        if let Some(request) = app.theme.request() {
            theme_jobs.spawn(async move {
                let result = request.execute().await;
                (request, result)
            });
        }
        if let Some(state) = &mut state {
            if app.closing && state.wait().is_some() {
                state.force();
            }
            state.start(&app);
        }
        if let Some(client) = &client {
            if let Some(request) = app.revision_request() {
                if request.needs_checkpoint() {
                    if let Some(state) = &mut state {
                        state.submit_revision(request);
                    } else {
                        app.revision_after_checkpoint(
                            &request,
                            &Err("TUI checkpoint unavailable".into()),
                        );
                    }
                } else {
                    let client = client.clone();
                    jobs.spawn(async move {
                        let result = pages::revision::execute(&client, &request).await;
                        Completed::Revised(request, result)
                    });
                }
                dirty = true;
            }
            if let Some(request) = app.branch_request() {
                if request.query {
                    let client = client.clone();
                    jobs.spawn(async move {
                        let result = pages::branch::execute(&client, &request).await;
                        Completed::Branched(request, result)
                    });
                } else if let Some(state) = &mut state {
                    state.submit_branch(request);
                } else {
                    app.branch_after_checkpoint(
                        &request,
                        &Err("TUI checkpoint unavailable".into()),
                    );
                }
                dirty = true;
            }
            if let Some(request) = app.oauth_request() {
                if request.needs_checkpoint() {
                    if let Some(state) = &mut state {
                        state.submit_oauth(request);
                    } else {
                        app.oauth_after_checkpoint(
                            &request,
                            &Err("TUI checkpoint unavailable".into()),
                        );
                    }
                    dirty = true;
                } else {
                    let client = client.clone();
                    jobs.spawn(async move {
                        let result = pages::manage::oauth::execute(&client, &request).await;
                        Completed::Oauth(request, result)
                    });
                }
            }
            if let Some(id) = app.chat.select(&app.navigation.current()) {
                close_observation(&mut jobs, client.clone(), id);
            }
            if history_job.is_none()
                && let Some(request) = app.chat.history_request()
            {
                let client = client.clone();
                history_job = Some(
                    jobs.spawn(async move {
                        let result = pages::chat::history::execute(&client, &request)
                            .await
                            .map_err(|error| error.to_string());
                        Completed::History(request, result)
                    })
                    .id(),
                );
            }
            if app.chat.error.is_some()
                && let Some(id) = app.chat.subscription.take()
            {
                close_observation(&mut jobs, client.clone(), id);
            }
            if let Some(request) = app.chat.open_query() {
                let client = client.clone();
                jobs.spawn(async move {
                    let result = pages::chat::open(&client, &request)
                        .await
                        .map(Box::new)
                        .map_err(|e| e.to_string());
                    Completed::ChatOpened(request, result)
                });
            }
            if let Some(request) = app.chat.context_query() {
                let client = client.clone();
                jobs.spawn(async move {
                    let result = async {
                        let value = client
                            .request(
                                Operation::ContextDiagnosticsQuery,
                                json!({"sessionId":request.session}),
                            )
                            .await?;
                        Ok::<_, Error>(maka_protocol::context::decode_context_diagnostics_result(
                            &value,
                        )?)
                    }
                    .await
                    .map_err(|error| error.to_string());
                    Completed::Context(request, result)
                });
            }
            if let Some(input) = app.sessions.query() {
                let client = client.clone();
                jobs.spawn(async move {
                    Completed::Catalog(
                        client
                            .session_catalog(input)
                            .await
                            .map_err(|e| e.to_string()),
                    )
                });
            }
            if let Some(input) = app.inbox.query() {
                let client = client.clone();
                jobs.spawn(async move {
                    Completed::Inbox(
                        client
                            .session_catalog(input)
                            .await
                            .map_err(|e| e.to_string()),
                    )
                });
            }
            if app.navigation.current() == navigation::Route::Projects
                && let Some(input) = app.projects.query()
            {
                let client = client.clone();
                jobs.spawn(async move {
                    Completed::Projects(
                        client
                            .project_catalog(input)
                            .await
                            .map_err(|e| e.to_string()),
                    )
                });
            }
            if app.navigation.current() == navigation::Route::Connections
                && let Some(input) = app.connections.query()
            {
                let client = client.clone();
                jobs.spawn(async move {
                    Completed::Connections(
                        client
                            .connection_catalog(input)
                            .await
                            .map_err(|e| e.to_string()),
                    )
                });
            }
            if let Some(request) = app.sessions.detail_query() {
                let client = client.clone();
                jobs.spawn(async move {
                    let result = client.session(&request.id).await.map_err(|e| e.to_string());
                    Completed::Session(request, result)
                });
            }
            if let Some(request) = app.directory_request() {
                let client = client.clone();
                jobs.spawn(async move {
                    let result = client
                        .project_catalog(request.query.clone())
                        .await
                        .map_err(|e| e.to_string());
                    Completed::Directory(request, result)
                });
            }
            if let Some(request) = app.choose_project_request() {
                let client = client.clone();
                jobs.spawn(async move {
                    let result = client
                        .project_catalog(request.query.clone())
                        .await
                        .map_err(|e| e.to_string());
                    Completed::ChooseProject(request, result)
                });
            }
            if let Some(request) = app.locations_request() {
                let client = client.clone();
                jobs.spawn(async move {
                    let result = client
                        .project_catalog(request.query.clone())
                        .await
                        .map_err(|e| e.to_string());
                    Completed::Locations(request, result)
                });
            }
            if let Some(request) = app.models_request() {
                let client = client.clone();
                jobs.spawn(async move {
                    let result = client
                        .connection_catalog(request.query.clone())
                        .await
                        .map_err(|e| e.to_string());
                    Completed::Models(request, result)
                });
            }
            if let Some(request) = app.enabled_models_request() {
                let client = client.clone();
                jobs.spawn(async move {
                    let result = client
                        .connection_catalog(request.query.clone())
                        .await
                        .map_err(|e| e.to_string());
                    Completed::EnabledModels(request, result)
                });
            }
            if let Some(request) = app.credential_request() {
                let client = client.clone();
                jobs.spawn(async move {
                    let result = client.credential_status(request.locator()).await;
                    Completed::Credential(request, result)
                });
            }
            if let Some(request) = app.removal_request() {
                let client = client.clone();
                jobs.spawn(async move {
                    let result = pages::manage::removal::read(&client, &request).await;
                    Completed::Removal(request, result)
                });
            }
        }
        if dirty {
            app.oauth_before_draw();
            app.sync_interaction();
            terminal::draw(&mut screen, |frame| view::draw(frame, &mut app))?;
            app.oauth_after_draw();
        }
        // Paging can depend on the just-measured viewport. Dispatch before
        // waiting for input, including when motion is disabled and the app is idle.
        if let Some(client) = &client
            && let Some(request) = app.chat.page_query()
        {
            let client = client.clone();
            jobs.spawn(async move {
                let result = async {
                    let page = client.transcript_page(request.input.clone()).await?;
                    client
                        .complete_transcript_page(&request.input.subscription_id, page)
                        .await
                }
                .await
                .map_err(|e: Error| e.to_string());
                Completed::ChatPage(request, result)
            });
        }
        if let Some(action) = effect.take() {
            match action {
                Action::Manage(pages::manage::Command::Oauth(command)) => {
                    app.oauth_copy(command);
                    dirty = true;
                    continue;
                }
                Action::Onboard(
                    command @ (pages::onboarding::Command::Verify
                    | pages::onboarding::Command::Save),
                ) => {
                    if let Some(client) = client.clone()
                        && let Some(request) =
                            app.onboarding_request(command == pages::onboarding::Command::Save)
                    {
                        jobs.spawn(async move {
                            let ticket = request.ticket.clone();
                            let result = pages::onboarding::execute(&client, request).await;
                            Completed::Onboard(ticket, result)
                        });
                    }
                    dirty = true;
                    continue;
                }
                Action::Manage(pages::manage::Command::Save) => {
                    if let Some(client) = client.clone()
                        && let Some(ticket) = app.management_request()
                    {
                        let secret = app.take_credential_secret(&ticket);
                        jobs.spawn(async move {
                            let result = pages::manage::execute(&client, &ticket, secret).await;
                            Completed::Managed(Box::new(ticket), result)
                        });
                    }
                    dirty = true;
                    continue;
                }
                Action::CopyFile(path) => {
                    let result = terminal::copy(&mut std::io::stdout(), &path);
                    app.notice = Some(Notice::Clipboard {
                        key: if result.is_ok() {
                            "chat-copy-requested"
                        } else {
                            "chat-copy-failed"
                        },
                        until: std::time::Instant::now() + Duration::from_secs(3),
                    });
                    dirty = true;
                    continue;
                }
                Action::Copy(mode) => {
                    let result = app
                        .chat
                        .reader()
                        .ok_or("chat-copy-empty")
                        .and_then(|reader| reader.copy_text(mode, app.chrome.ascii))
                        .and_then(|text| {
                            terminal::copy(&mut std::io::stdout(), &text)
                                .map_err(|_| "chat-copy-failed")
                        });
                    app.notice = Some(Notice::Clipboard {
                        key: result.map_or_else(|key| key, |_| "chat-copy-requested"),
                        until: std::time::Instant::now() + Duration::from_secs(3),
                    });
                    dirty = true;
                    continue;
                }
                Action::Interaction(command) => {
                    if let Some((ticket, answer)) = app.interaction_request(command)
                        && let Some(client) = client.clone()
                    {
                        jobs.spawn(async move {
                            let result = if let Some(answer) = answer {
                                client.answer_interaction(&ticket.snapshot, answer).await
                            } else {
                                client.interaction(&ticket.snapshot).await
                            };
                            Completed::Interaction(Box::new((ticket, result)))
                        });
                    }
                    dirty = true;
                    continue;
                }
                Action::Quit => {
                    app.branch.disconnect();
                    app.revision.disconnect();
                    if let Some(state) = &mut state {
                        app.oauth_abandon_checkpoint();
                        for request in state.cancel_requests() {
                            app.abandon_checkpoint(&request);
                        }
                        state.force();
                        app.closing = true;
                        dirty = true;
                        continue;
                    }
                    break;
                }
                Action::Connect => {
                    app.branch.disconnect();
                    app.revision.disconnect();
                    if let Some(state) = &mut state {
                        state.cancel_requests();
                    }
                    app.queue.abandon();
                    app.abandon_management();
                    app.management.oauth.abandon();
                    app.abandon_onboarding();
                    app.abandon_interaction();
                    app.abandon_pending_submissions();
                    app.creating = false;
                    jobs = JoinSet::new();
                    history_job = None;
                    app.chat.reset();
                    if let Some(old) = client.take() {
                        old.disconnect();
                    }
                    notifications = None;
                    oauth_service = None;
                    app.refreshing = false;
                    let root = app.root.clone();
                    jobs.spawn(async move { Completed::Connected(connect(root).await) });
                }
                Action::StopTurn(target) => {
                    if let Some(client) = client.clone()
                        && client.identity.root_id == target.root
                        && client.identity.host_epoch == target.epoch
                        && app.stop_target().as_ref() == Some(&target)
                        && app.chat.start_stop(&target)
                    {
                        jobs.spawn(async move {
                            let result = client.stop_turn(target.input()).await;
                            Completed::Stopped(target, result)
                        });
                    }
                    dirty = true;
                    continue;
                }
                Action::Queue(command) => {
                    if let Some(client) = client.clone()
                        && let Some(ticket) = app.queue_request(command)
                    {
                        jobs.spawn(async move {
                            let result = pages::queue::execute(&client, &ticket).await;
                            Completed::Queue(ticket, result)
                        });
                    }
                    dirty = true;
                    continue;
                }
                Action::SendMessage | Action::SteerMessage | Action::RetrySubmission => {
                    if client.is_some()
                        && let Some(request) = if action == Action::RetrySubmission {
                            app.retry_submission()
                        } else if action == Action::SteerMessage {
                            app.submission_for(maka_protocol::message::Placement::CurrentTurn)
                        } else {
                            app.submission()
                        }
                    {
                        if let Some(state) = &mut state {
                            state.submit(request);
                        } else {
                            let error = app.i18n.text("state-unavailable");
                            app.after_checkpoint(&request, &Err(error.clone()));
                            app.state_error = Some(error);
                        }
                    }
                    dirty = true;
                    continue;
                }
                Action::ReconcileSubmission => {
                    if let Some(client) = client.clone()
                        && let Some(request) = app.reconciliation()
                    {
                        jobs.spawn(async move {
                            let result = client
                                .message_execution(&request.session, &request.id)
                                .await;
                            Completed::Reconciled(request, result)
                        });
                    }
                    dirty = true;
                    continue;
                }
                Action::CreateSession | Action::Project(pages::projects::Command::Create(_)) => {
                    if let Some(client) = client.clone() {
                        let name = app.i18n.text("session-new");
                        let origin = app.navigation.current();
                        jobs.spawn(async move {
                            let result = async {
                                let workspace = match action {
                                    Action::Project(pages::projects::Command::Create(id)) => {
                                        json!({"kind":"project", "projectId":id})
                                    }
                                    _ => {
                                        json!({"kind":"host_path","path":std::env::current_dir()?})
                                    }
                                };
                                let input =
                                    maka_protocol::session::decode_session_create_input(&json!({
                                        "sessionId":uuid::Uuid::new_v4().to_string(), "name":name,
                                        "workspace":workspace,
                                        "modelTarget":{"kind":"default"}
                                    }))?;
                                Ok::<_, Error>(Box::new(client.create_session(input).await?))
                            }
                            .await
                            .map_err(|e| e.to_string());
                            Completed::Created(origin, result)
                        });
                    }
                    dirty = true;
                    continue;
                }
                Action::Refresh => {
                    if let Some(client) = client.clone() {
                        jobs.spawn(async move {
                            Completed::Status(
                                client
                                    .request(Operation::HostStatus, json!({}))
                                    .await
                                    .map_err(|e| e.to_string()),
                            )
                        });
                    }
                }
                Action::RefreshSession => {
                    if let Some(id) = app.chat.refresh()
                        && let Some(client) = client.clone()
                    {
                        close_observation(&mut jobs, client, id);
                    }
                    dirty = true;
                    continue;
                }
                _ => {}
            }
        }
        let state_wait = state.as_ref().and_then(state::State::wait);
        let oauth_wait = app.oauth_wait();
        tokio::select! {
            _ = async {
                match oauth_wait {
                    Some(wait) => tokio::time::sleep(wait).await,
                    None => std::future::pending().await,
                }
            } => {}
            request = async {
                match oauth_service.as_mut() {
                    Some(service) => service.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(request) = request { app.oauth_presentation(request); }
                else { oauth_service = None; app.oauth_service_closed(); }
                dirty = true;
            }
            _ = async {
                match state_wait {
                    Some(wait) => tokio::time::sleep(wait).await,
                    None => std::future::pending().await,
                }
            } => {}
            written = async {
                match &mut state {
                    Some(state) => state.completed().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(request) = written.revision
                    && app.revision_after_checkpoint(&request, &written.result)
                    && let Some(client) = client.clone() {
                    jobs.spawn(async move {
                        let result = pages::revision::execute(&client, &request).await;
                        Completed::Revised(request, result)
                    });
                }
                if let Some(request) = written.branch
                    && app.branch_after_checkpoint(&request, &written.result)
                    && let Some(client) = client.clone() {
                    jobs.spawn(async move {
                        let result = pages::branch::execute(&client, &request).await;
                        Completed::Branched(request, result)
                    });
                }
                if let Some(request) = written.oauth
                    && app.oauth_after_checkpoint(&request, &written.result)
                    && let Some(client) = client.clone() {
                    jobs.spawn(async move {
                        let result = pages::manage::oauth::execute(&client, &request).await;
                        Completed::Oauth(request, result)
                    });
                }
                for request in written.requests {
                    if app.after_checkpoint(&request, &written.result)
                        && let Some(client) = client.clone() {
                        jobs.spawn(async move {
                            let result = client.submit_message(request.input()).await;
                            Completed::Submitted(request, result)
                        });
                    }
                }
                app.state_error = written.result.err();
                if app.state_error.is_some() { app.closing = false; }
                if app.closing && state.as_ref().is_some_and(state::State::idle) {
                    flushed = true;
                    break;
                }
                dirty = true;
            }
            _ = async {
                match app.selection_wait(std::time::Instant::now()) {
                    Some(wait) => tokio::time::sleep(wait).await,
                    None => std::future::pending().await,
                }
            } => {
                dirty = app.selection_scroll(std::time::Instant::now());
                if dirty && let Some(state) = &mut state { state.changed(); }
            }
            _ = async {
                match &app.notice {
                    Some(Notice::Clipboard { until, .. }) => tokio::time::sleep(until.saturating_duration_since(std::time::Instant::now())).await,
                    _ => std::future::pending().await,
                }
            } => { app.notice = None; dirty = true; }
            _ = async {
                let wait = app.chat.view.search.as_ref().and_then(|search| search.history.as_ref())
                    .and_then(|history| history.wait())
                    .filter(|_| history_job.is_none() && client.is_some() && app.chat.error.is_none() && app.chat.snapshot.is_some());
                match wait { Some(wait) => tokio::time::sleep(wait).await, None => std::future::pending().await }
            } => {}
            _ = async {
                match app.chat.context.wait().filter(|_| client.is_some() && app.chat.error.is_none() && app.chat.snapshot.is_some()) {
                    Some(wait) => tokio::time::sleep(wait).await,
                    None => std::future::pending().await,
                }
            } => {}
            _ = async {
                match app.tooltip_wait() {
                    Some(wait) => tokio::time::sleep(wait).await,
                    None => std::future::pending::<()>().await,
                }
            } => { dirty = true; }
            _ = async {
                let now=std::time::Instant::now();
                let wait=if app.chrome.animating() { Some(Duration::from_millis(16)) }
                    else { app.chrome.animation.wait(now) };
                match wait { Some(wait)=>tokio::time::sleep(wait).await, None=>std::future::pending().await }
            } => {
                dirty = true;
            }
            event = input.next() => {
                match event {
                    Some(Ok(event)) => {
                        (dirty, effect) = app.input(event);
                        if dirty && let Some(state) = &mut state { state.changed(); }
                    }
                    Some(Err(error)) => return Err(error.into()),
                    None => break,
                }
            }
            completed = theme_jobs.join_next(), if !theme_jobs.is_empty() => {
                if let Some(completed) = completed {
                    let (request, result) = completed?;
                    app.theme.complete(request, result);
                    if let Some(state) = &mut state { state.changed(); }
                    dirty = true;
                }
            }
            completed = jobs.join_next(), if !jobs.is_empty() => {
                match completed {
                    Some(Ok(Completed::Revised(request, result))) => {
                        app.revision_completed(request, result);
                        if let Some(state) = &mut state { state.changed(); }
                    }
                    Some(Ok(Completed::Branched(request, result))) => {
                        app.branch_completed(request, result);
                        if let Some(state) = &mut state { state.changed(); }
                    }
                    Some(Ok(Completed::Oauth(request, result))) => {
                        if let Some(service) = app.oauth_completed(request, result) {
                            oauth_service = Some(service);
                        }
                        if let Some(state) = &mut state { state.changed(); }
                    }
                    Some(Ok(Completed::Managed(ticket, result))) => {
                        app.management_completed(*ticket, result);
                        if let Some(state) = &mut state { state.changed(); }
                    },
                    Some(Ok(Completed::Removal(request, result))) => {
                        app.removal_read(request, result);
                        if let Some(state) = &mut state { state.changed(); }
                    },
                    Some(Ok(Completed::Directory(request, result))) => app.directory_completed(request, result),
                    Some(Ok(Completed::ChooseProject(request, result))) => app.choose_project_completed(request, result),
                    Some(Ok(Completed::Locations(request,result))) => app.locations_completed(request,result),
                    Some(Ok(Completed::Models(request,result)))=>app.models_completed(request,result),
                    Some(Ok(Completed::EnabledModels(request,result)))=>app.enabled_models_completed(request,result),
                    Some(Ok(Completed::Credential(request,result)))=>app.credential_completed(request,result),
                    Some(Ok(Completed::Onboard(ticket,result)))=>app.onboarding_completed(ticket,result),
                    Some(Ok(Completed::History(request, result))) => {
                        history_job = None;
                        app.chat.history_completed(request, result, &app.i18n, app.chrome.ascii);
                    }
                    Some(Ok(Completed::Queue(ticket, result))) => app.queue_completed(ticket, result),
                    Some(Ok(Completed::Stopped(target, result))) => app.chat.stopped(target, result),
                    Some(Ok(Completed::Context(request, result))) => app.chat.context_completed(request, result),
                    Some(Ok(Completed::Interaction(result))) => {
                        let (ticket, result) = *result;
                        app.interaction_completed(ticket, result);
                    }
                    Some(Ok(Completed::Reconciled(request, result))) => {
                        app.reconciled(request, result);
                        if let Some(state) = &mut state { state.changed(); }
                    }
                    Some(Ok(Completed::Created(origin, result))) => {
                        app.creating = false;
                        match result {
                            Ok(session) => {
                                app.sessions.refresh();
                                if app.navigation.current() == origin {
                                    app.apply(Action::Visit(navigation::Route::Session(session.id)));
                                }
                                if let Some(state) = &mut state { state.changed(); }
                            }
                            Err(error) => app.notice = Some(Notice::Diagnostic(error)),
                        }
                    }
                    Some(Ok(Completed::Submitted(request, result))) => {
                        app.submitted(request, result);
                        if let Some(state) = &mut state { state.changed(); }
                    }
                    Some(Ok(Completed::ChatOpened(request, result))) => {
                        if let Some(id) = app.chat.opened(request, result.map(|opened| *opened)) {
                            if let Some(client) = client.clone() { close_observation(&mut jobs, client, id); }
                        } else if app.chat.error.is_none()
                            && let Some(id) = app.chat.subscription.clone()
                            && let Some(client) = client.clone() {
                            jobs.spawn(async move {
                                let result = client.ready_subscription(&id).await.map_err(|e| e.to_string());
                                Completed::ChatReady(id, result)
                            });
                        }
                        if let Some(state) = &mut state { state.changed(); }
                    }
                    Some(Ok(Completed::ChatPage(request, result))) => {
                        app.chat.page(request, result);
                        if let Some(state) = &mut state { state.changed(); }
                    }
                    Some(Ok(Completed::ChatReady(id, Err(error)))) if app.chat.subscription.as_ref() == Some(&id) => app.chat.error = Some(error),
                    Some(Ok(Completed::Connected(Ok((connected, receiver))))) => {
                        if state.is_none() {
                            match state::State::open(&app.root, &options.profile).await {
                                Ok(Some((opened, saved))) => {
                                    if let Some(saved) = saved { saved.restore(&mut app, keep_locale)?; }
                                    state = Some(opened);
                                }
                                Ok(None) => {}
                                Err(error) => {
                                    connected.disconnect();
                                    return Err(app.i18n.format("state-open-failed", &[("error", &error.to_string())]).into());
                                }
                            }
                        }
                        if !app.bind_root(&connected.identity.root_id) {
                            connected.disconnect();
                            app.connection = ConnectionState::Failed(app.i18n.text("host-root-changed"));
                            dirty = true;
                            continue;
                        }
                        app.connection = ConnectionState::Connected {
                            root_id: connected.identity.root_id.clone(),
                            epoch: connected.identity.host_epoch.clone(),
                        };
                        client = Some(connected);
                        notifications = Some(receiver);
                        app.sessions.refresh();
                        app.inbox.refresh();
                        app.projects.refresh();
                        app.connections.refresh();
                        if let navigation::Route::Session(id) = app.navigation.current() { app.sessions.open(&id); }
                        effect = app.apply(Action::Refresh);
                    }
                    Some(Ok(Completed::Connected(Err(error)))) => app.connection = ConnectionState::Failed(error.to_string()),
                    Some(Ok(Completed::Catalog(result))) => app.sessions.complete(result),
                    Some(Ok(Completed::Inbox(result))) => app.inbox.complete(result),
                    Some(Ok(Completed::Projects(result))) => app.projects.complete(result),
                    Some(Ok(Completed::Connections(result))) => app.connections.complete(result),
                    Some(Ok(Completed::Session(request, result))) => {
                        app.sessions.complete_detail(request, result);
                        if let pages::sessions::Detail::Missing { id } = &app.sessions.detail {
                            let id = id.clone();
                            app.session_removed(&id, false);
                        }
                    },
                    Some(Ok(Completed::Status(result))) => {
                        app.refreshing = false;
                        match result {
                            Ok(status) if client.as_ref().is_some_and(|c| status["hostEpoch"] == c.identity.host_epoch) => app.status = Some(status),
                            Ok(_) => {
                                if let Some(old) = client.take() { old.disconnect(); }
                                app.connection = ConnectionState::WrongEpoch;
                            }
                            Err(error) => app.notice = Some(Notice::Diagnostic(error)),
                        }
                    }
                    Some(Err(error)) if history_job == Some(error.id()) => {
                        history_job = None;
                        if let Some(history) = app.chat.view.search.as_mut().and_then(|search| search.history.as_mut()) {
                            history.fail(error.to_string());
                        }
                    }
                    Some(Err(error)) if !error.is_cancelled() => app.notice = Some(Notice::Diagnostic(error.to_string())),
                    _ => {}
                }
                dirty = true;
            }
            notice = async {
                match notifications.as_mut() {
                    Some(receiver) => receiver.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                match notice {
                    Some(Notification::Catalog(notice)) => {
                        if notice.kind == "project.catalog.changed" { app.project_catalog_changed(); }
                        if matches!(notice.kind.as_str(), "connection.catalog.changed" | "configuration.changed") {
                            app.models_catalog_changed();
                            app.connections.refresh();
                            app.chat.context.refresh();
                        }
                        if notice.kind == "session.catalog.changed"
                            && let Some(id) = &notice.session_id {
                                app.sessions.invalidate(id);
                                app.inbox.refresh();
                            }
                        app.notice = Some(Notice::Catalog { kind: notice.kind, revision: notice.revision.to_string() });
                    }
                    Some(Notification::Observation(frame)) => {
                        if let Err(error) = app.chat.accept(*frame) {
                            app.chat.error = Some(error.to_string());
                            if let Some(client) = &client { client.disconnect(); }
                        }
                        if app.chat.removed && let Some(id) = app.chat.session.clone() {
                            app.session_removed(&id, false);
                        }
                    }
                    None => notifications = None,
                }
                dirty = true;
            }
            error = async {
                match client.as_ref() {
                    Some(client) => client.closed().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(state) = &mut state { state.cancel_requests(); }
                jobs = JoinSet::new();
                history_job = None;
                app.abandon_interaction();
                app.abandon_pending_submissions();
                app.abandon_management();
                app.branch.disconnect();
                    app.revision.disconnect();
                app.management.oauth.abandon();
                app.abandon_onboarding();
                app.creating = false;
                client = None;
                notifications = None;
                oauth_service = None;
                app.status = None;
                app.refreshing = false;
                app.connection = ConnectionState::Failed(error.to_string());
                app.chat.error = Some(error.to_string());
                dirty = true;
            }
            _ = termination_signal(
                #[cfg(unix)]
                &mut terminate
            ) => break,
        }
    }
    jobs.abort_all();
    // Once Save was pressed, finish the bounded local write and checkpoint the
    // resulting choice before exit; dropping Host jobs must not strand it.
    while let Some(completed) = theme_jobs.join_next().await {
        let (request, result) = completed?;
        app.theme.complete(request, result);
        flushed = false;
    }
    if let Some(client) = client {
        client.disconnect();
    }
    if !flushed && let Some(state) = &mut state {
        state.finish(&app).await?;
    }
    Ok(())
}

fn close_observation(jobs: &mut JoinSet<Completed>, client: Client, id: String) {
    jobs.spawn(async move {
        if client.close_subscription(&id).await.is_err() {
            client.disconnect();
        }
        Completed::ObservationClosed
    });
}

async fn connect(root: PathBuf) -> Result<(Client, mpsc::Receiver<Notification>), Error> {
    tokio::time::timeout(Duration::from_secs(6), async {
        let discovery =
            tokio::task::spawn_blocking(move || maka_client::local::read_discovery(&root))
                .await??;
        let stream = maka_client::local::open_stream(&discovery.endpoint).await?;
        Ok(Client::connect(
            stream,
            &discovery.root_id,
            &discovery.host_epoch,
            maka_client::Operations,
        )
        .await?)
    })
    .await?
}

async fn termination_signal(#[cfg(unix)] signal: &mut tokio::signal::unix::Signal) {
    #[cfg(unix)]
    {
        signal.recv().await;
    }
    #[cfg(not(unix))]
    {
        std::future::pending::<()>().await;
    }
}

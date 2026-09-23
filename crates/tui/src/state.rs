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

mod snapshot;
mod store;

use crate::{
    app::App,
    pages::{manage::oauth, sending::Submission},
};
use maka_client::Error;
use snapshot::Snapshot;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub struct State {
    root: String,
    store: Arc<Mutex<store::Store>>,
    deadline: Option<Instant>,
    requests: Vec<Submission>,
    oauth: Option<oauth::Request>,
    generation: u64,
    job: Option<Writing>,
}

struct Writing {
    task: tokio::task::JoinHandle<Result<(), String>>,
    requests: Vec<Submission>,
    oauth: Option<oauth::Request>,
    generation: u64,
}

pub struct Written {
    pub result: Result<(), String>,
    pub requests: Vec<Submission>,
    pub oauth: Option<oauth::Request>,
}
impl State {
    pub async fn open(
        root: &Path,
        profile: &str,
    ) -> Result<Option<(Self, Option<Snapshot>)>, Error> {
        let root = root.to_owned();
        let profile = profile.to_owned();
        tokio::task::spawn_blocking(move || {
            // Missing/invalid Host roots remain diagnosable in the normal UI.
            // Do not create a Host Root or a path-keyed substitute identity.
            let Ok(location) = maka_event_log::root::resolve(&root) else {
                return Ok(None);
            };
            let base = match std::env::var_os("MAKA_TUI_STATE_DIR") {
                Some(base) => PathBuf::from(base),
                None => maka_event_log::root::RootNamespaces::for_current_account()?
                    .ownership
                    .parent()
                    .ok_or("missing account directory")?
                    .join("tui"),
            };
            let (store, saved) = store::Store::open(&base, location.root_id(), &profile)?;
            Ok::<_, Error>(Some((
                Self {
                    root: store.root.clone(),
                    store: Arc::new(Mutex::new(store)),
                    deadline: None,
                    requests: Vec::new(),
                    oauth: None,
                    generation: 0,
                    job: None,
                },
                saved,
            )))
        })
        .await?
    }
    pub fn changed(&mut self) {
        self.deadline
            .get_or_insert_with(|| Instant::now() + Duration::from_millis(500));
    }
    pub fn wait(&self) -> Option<Duration> {
        if self.job.is_some() {
            return None;
        }
        self.deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }
    pub fn force(&mut self) {
        self.deadline = Some(Instant::now());
    }
    pub fn submit(&mut self, request: Submission) {
        // App permits at most one unresolved request per bounded draft slot.
        self.requests
            .retain(|pending| pending.session != request.session);
        self.requests.push(request);
        self.force();
    }
    pub fn cancel_requests(&mut self) -> Vec<Submission> {
        self.oauth = None;
        let mut requests = std::mem::take(&mut self.requests);
        if let Some(job) = &self.job
            && job.generation == self.generation
        {
            requests.extend(job.requests.iter().cloned());
        }
        self.generation += 1;
        requests
    }
    pub fn submit_oauth(&mut self, request: oauth::Request) {
        self.oauth = Some(request);
        self.force();
    }
    pub fn idle(&self) -> bool {
        self.job.is_none() && self.deadline.is_none()
    }
    pub fn start(&mut self, app: &App) {
        if self.wait() != Some(Duration::ZERO) {
            return;
        }
        self.deadline = None;
        let snapshot = Snapshot::capture(app, &self.root);
        let store = self.store.clone();
        let requests = std::mem::take(&mut self.requests);
        let generation = self.generation;
        self.job = Some(Writing {
            task: tokio::task::spawn_blocking(move || {
                store
                    .lock()
                    .map_err(|_| "TUI state writer failed".to_owned())
                    .and_then(|mut store| store.save(snapshot).map_err(|e| e.to_string()))
            }),
            requests,
            oauth: self.oauth.take(),
            generation,
        });
    }
    /// Cancellation-safe: select! may stop waiting, but never detaches the writer.
    pub async fn completed(&mut self) -> Written {
        let Some(job) = &mut self.job else {
            return std::future::pending().await;
        };
        let result = (&mut job.task)
            .await
            .unwrap_or_else(|error| Err(error.to_string()));
        let job = self.job.take().expect("completed writer");
        Written {
            result,
            requests: if job.generation == self.generation {
                job.requests
            } else {
                Vec::new()
            },
            oauth: if job.generation == self.generation {
                job.oauth
            } else {
                None
            },
        }
    }
    /// Terminal EOF/signals have no interactive loop to keep responsive.
    pub async fn finish(&mut self, app: &App) -> Result<(), String> {
        self.cancel_requests();
        if self.job.is_some() {
            self.completed().await;
        }
        self.force();
        self.start(app);
        self.completed().await.result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::{Action, ConnectionState},
        i18n::{I18n, Locale, LocalePreference},
        navigation::Route,
    };
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    const ROOT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    fn fixture() -> (tempfile::TempDir, State, App) {
        let directory = tempfile::tempdir().unwrap();
        let (store, _) = store::Store::open(directory.path(), ROOT, "default").unwrap();
        let state = State {
            root: ROOT.into(),
            store: Arc::new(Mutex::new(store)),
            deadline: None,
            requests: vec![],
            oauth: None,
            generation: 0,
            job: None,
        };
        let mut app = App::new(
            "/unused".into(),
            I18n::new(LocalePreference::Auto, Locale::En),
        );
        app.connection = ConnectionState::Connected {
            root_id: ROOT.into(),
            epoch: "epoch".into(),
        };
        app.apply(Action::Visit(Route::Session("a".into())));
        (directory, state, app)
    }
    async fn gate(state: &State) -> (std::sync::mpsc::Sender<()>, tokio::task::JoinHandle<()>) {
        let store = state.store.clone();
        let (ready, started) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let task = tokio::task::spawn_blocking(move || {
            let _guard = store.lock().unwrap();
            ready.send(()).unwrap();
            let _ = wait.recv(); // Dropping the sender also releases the gate on test failure.
        });
        started.await.unwrap();
        (release, task)
    }
    async fn written(state: &mut State) -> Written {
        tokio::time::timeout(Duration::from_secs(5), state.completed())
            .await
            .unwrap()
    }
    fn read(directory: &tempfile::TempDir) -> serde_json::Value {
        serde_json::from_slice(
            &std::fs::read(directory.path().join(ROOT).join("default/state.json")).unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn slow_writer_keeps_input_live_coalesces_edits_and_never_overwrites_newer_state() {
        let (directory, mut state, mut app) = fixture();
        app.input(Event::Paste("original 中文🦀".into()));
        let request = app.submission().unwrap();
        let (release, blocked) = gate(&state).await;
        state.submit(request.clone());
        state.start(&app);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), state.completed())
                .await
                .is_err()
        );
        for _ in 0..100 {
            assert!(app.input(Event::Paste("x".into())).0);
            state.changed();
            state.start(&app);
        }
        assert!(state.requests.is_empty());
        app.apply(Action::Visit(Route::Session("b".into())));
        app.input(Event::Paste("second session".into()));
        let second = app.submission().unwrap();
        state.submit(second.clone());
        state.start(&app);
        assert_eq!(
            state.requests.len(),
            1,
            "a later send waits for the next checkpoint"
        );
        assert!(
            state.wait().is_none(),
            "no busy timer while an IO worker is blocked"
        );
        assert!(!state.idle());
        app.apply(Action::Visit(Route::Settings));
        state.changed();
        let mut screen =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        screen
            .draw(|frame| crate::view::draw(frame, &mut app))
            .unwrap();
        assert_eq!(app.navigation.current(), Route::Settings);
        release.send(()).unwrap();
        blocked.await.unwrap();
        let first = written(&mut state).await;
        assert!(first.result.is_ok());
        assert_eq!(first.requests, vec![request.clone()]);
        assert!(app.after_checkpoint(&request, &first.result));
        assert_eq!(read(&directory)["drafts"]["a"]["text"], "original 中文🦀");
        state.force();
        state.start(&app);
        let latest = written(&mut state).await;
        assert!(latest.result.is_ok());
        assert_eq!(latest.requests, vec![second.clone()]);
        assert!(app.after_checkpoint(&second, &latest.result));
        assert_eq!(
            read(&directory)["drafts"]["a"]["text"],
            app.drafts["a"].text()
        );
        assert!(state.idle());

        // Closing is a cancellable input scope, not a blocking disk wait.
        app.closing = true;
        assert!(!app.input(Event::Paste("must not edit".into())).0);
        assert!(app.input(Event::Resize(100, 30)).0);
        app.input(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(!app.closing);
    }

    #[tokio::test]
    async fn cancelled_attempt_ack_cannot_release_a_new_retry_and_failed_checkpoint_never_dispatches()
     {
        let (directory, mut state, mut app) = fixture();
        app.input(Event::Paste("original".into()));
        let request = app.submission().unwrap();
        let (release, blocked) = gate(&state).await;
        state.submit(request.clone());
        state.start(&app);
        // Same Root and epoch after reconnect: the exact same submission may be retried.
        state.cancel_requests();
        app.abandon_pending_submissions();
        state.submit(app.retry_submission().unwrap());
        release.send(()).unwrap();
        blocked.await.unwrap();
        assert!(
            written(&mut state).await.requests.is_empty(),
            "old acknowledgement cannot dispatch the new attempt"
        );
        state.start(&app);
        let retried = written(&mut state).await;
        assert_eq!(retried.requests, vec![request.clone()]);
        assert!(app.after_checkpoint(&request, &retried.result));

        // A directory in place of the target makes atomic replacement fail on all platforms.
        let path = directory.path().join(ROOT).join("default/state.json");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        state.submit(request.clone());
        state.start(&app);
        let failed = written(&mut state).await;
        assert!(failed.result.is_err());
        assert!(!app.after_checkpoint(&request, &failed.result));
        assert!(matches!(
            app.sending["a"].delivery,
            crate::pages::sending::Delivery::Unknown(Some(_))
        ));
        assert_eq!(app.drafts["a"].text(), "original");
        assert!(path.is_dir());
        assert!(state.idle(), "failed writes do not retry in a hot loop");
        app.retry_submission().unwrap();
        app.connection = ConnectionState::Connected {
            root_id: ROOT.into(),
            epoch: "next-epoch".into(),
        };
        assert!(
            !app.after_checkpoint(&request, &Ok(())),
            "a successful disk write cannot rebind the Host epoch"
        );
        std::fs::remove_dir(&path).unwrap();
        state.finish(&app).await.unwrap();
        assert_eq!(read(&directory)["unresolved"][0]["id"], request.id);
        assert!(state.idle());
    }
}

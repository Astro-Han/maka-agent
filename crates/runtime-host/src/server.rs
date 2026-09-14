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

mod access;
mod artifacts;
mod authority;
mod bootstrap;
pub(crate) mod capabilities;
mod catalog_feed;
pub(crate) mod configuration;
mod connection;
mod connection_effects;
mod context;
mod dispatch;
mod execution_boundary;
mod handshake;
pub(crate) mod interactions;
mod listeners;
pub mod local;
pub(crate) mod messages;
mod navigation;
mod oauth;
mod onboarding;
mod operations;
mod outbound;
mod projects;
mod registration;
mod resources;
mod sessions;
mod skills;
mod subscriptions;
mod turns;
pub mod websocket;
mod workhub;

use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use maka_config::ConfigurationStore;
use maka_event_log::EventLog;
use maka_event_log::root::RootOwner;
use maka_protocol::handshake::Lifecycle;
use maka_transport::{MessageReader, MessageWriter};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use uuid::Uuid;

pub type HostError = Box<dyn Error + Send + Sync>;
pub use projects::DirectoryRootSpec;
pub use registration::Registration;

pub struct HostOptions {
    pub project_directory_roots: Option<Vec<DirectoryRootSpec>>,
    /// Explicit user skill root; library embedders do not inspect ambient home.
    pub skill_home: Option<std::path::PathBuf>,
    pub generation: Option<String>,
    pub handshake_timeout: Duration,
}

impl Default for HostOptions {
    fn default() -> Self {
        Self {
            project_directory_roots: None,
            skill_home: None,
            generation: None,
            handshake_timeout: Duration::from_secs(2),
        }
    }
}

pub struct Host {
    options: HostOptions,
    handshake_gate: Mutex<bool>,
    accepted_connections: AtomicUsize,
    started: std::time::Instant,
    accepted_connection: AtomicBool,
    executions: Arc<crate::execution::Executions>,
    shells: Arc<crate::shell::ShellResources>,
    controllers: crate::controllers::Controllers,
    capabilities: Arc<capabilities::Capabilities>,
    interactions: Arc<interactions::Interactions>,
    uploads: artifacts::Uploads,
    project_directories: projects::Directories,
    // Log connections close before root authority is released.
    log: Arc<EventLog>,
    configuration: Arc<ConfigurationStore>,
    connection_effects: connection_effects::ConnectionEffects,
    oauth: oauth::Coordinator,
    changes: broadcast::Sender<serde_json::Value>,
    change_revision: AtomicU64,
    access_revocations: broadcast::Sender<String>,
    access_changed: tokio::sync::Notify,
    session_catalog: catalog_feed::CatalogFeed,
    subscriptions: subscriptions::Registry,
    transcript_budget: Arc<tokio::sync::Semaphore>,
    root: Arc<RootOwner>,
    epoch: String,
    connections: AtomicUsize,
    draining: CancellationToken,
    requests: TaskTracker,
}

impl Host {
    pub async fn open(root: RootOwner) -> Result<Arc<Self>, HostError> {
        Self::open_with_global_instructions(root, None).await
    }

    /// Embedders explicitly supply user-global instructions; no ambient home
    /// directory is consulted by the library. The standalone CLI supplies it.
    pub async fn open_with_global_instructions(
        root: RootOwner,
        global_instructions: Option<std::path::PathBuf>,
    ) -> Result<Arc<Self>, HostError> {
        Self::open_with_options(root, global_instructions, HostOptions::default()).await
    }

    pub async fn open_with_options(
        root: RootOwner,
        global_instructions: Option<std::path::PathBuf>,
        mut options: HostOptions,
    ) -> Result<Arc<Self>, HostError> {
        if let Some(generation) = &options.generation {
            maka_protocol::codec::string(&serde_json::json!(generation), "generation", 128)?;
        }
        if options.handshake_timeout.is_zero()
            || options.handshake_timeout > Duration::from_secs(300)
        {
            return Err("handshake timeout must be in (0, 300s]".into());
        }
        root.validate_current()?;
        if options
            .skill_home
            .as_ref()
            .is_some_and(|home| !home.is_absolute())
        {
            return Err("Skill home must be an absolute directory".into());
        }
        let project_directories =
            projects::Directories::open(options.project_directory_roots.take())
                .map_err(|error| error.message)?;
        let root = Arc::new(root);
        access::purge(&root).await?;
        let log = Arc::new(EventLog::for_root(root.clone()).await?);
        maka_agent::recovery::recover(&log).await?;
        let recovered_at = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis(),
        )?;
        log.recover_shell_runs(recovered_at).await?;
        log.recover_workhub_stops().await?;
        let configuration = Arc::new(ConfigurationStore::for_root(root.clone()).await?);
        let draining = CancellationToken::new();
        let startup_guard = draining.clone().drop_guard();
        let capabilities = Arc::new(capabilities::Capabilities::default());
        let epoch = Uuid::new_v4().to_string();
        log.begin_message_epoch(&epoch).await?;
        let interactions = Arc::new(interactions::Interactions::new(
            log.clone(),
            draining.clone(),
            epoch.clone(),
        ));
        let runtime = maka_js_runtime::trusted::TrustedRuntime::default();
        let executions = Arc::new(crate::execution::Executions::new(
            log.clone(),
            configuration.clone(),
            draining.clone(),
            capabilities.clone(),
            interactions.clone(),
            crate::execution::ExecutionPaths {
                state_root: root.canonical_path().to_owned(),
                global_instructions,
                skill_home: options.skill_home.take(),
            },
            runtime.clone(),
        )?);
        let session_catalog = catalog_feed::CatalogFeed::new(*log.subscribe_commits().borrow());
        let host = Arc::new(Self {
            options,
            handshake_gate: Mutex::new(false),
            accepted_connections: AtomicUsize::new(0),
            started: std::time::Instant::now(),
            accepted_connection: AtomicBool::new(false),
            shells: executions.shells.clone(),
            controllers: executions.controllers.clone(),
            executions,
            capabilities,
            interactions,
            uploads: Default::default(),
            project_directories,
            log,
            configuration,
            connection_effects: connection_effects::ConnectionEffects::default(),
            oauth: oauth::Coordinator::default(),
            changes: broadcast::channel(64).0,
            change_revision: AtomicU64::new(0),
            access_revocations: broadcast::channel(64).0,
            access_changed: tokio::sync::Notify::new(),
            session_catalog,
            subscriptions: Default::default(),
            transcript_budget: Arc::new(tokio::sync::Semaphore::new(64 * 1024 * 1024)),
            root,
            epoch,
            connections: AtomicUsize::new(0),
            draining,
            requests: TaskTracker::new(),
        });
        let recovery = async {
            workhub::recover(&host).await?;
            host.executions.recover_messages().await
        }
        .await;
        if let Err(error) = recovery {
            host.capabilities.begin_drain();
            host.executions.shutdown().await;
            host.shells.shutdown().await;
            host.capabilities.shutdown().await;
            let _ = tokio::join!(host.log.shutdown(), host.configuration.shutdown());
            return Err(error.message.into());
        }
        startup_guard.disarm();
        Ok(host)
    }

    pub fn root_id(&self) -> &str {
        self.root.root_id()
    }
    pub fn control_directory(&self) -> &std::path::Path {
        self.root.control_directory()
    }

    /// Candidate expiry observes accepted work; a disconnected model or PTY still owns residency.
    pub async fn wait_until_idle(&self, initial_timeout: Duration, idle_grace: Duration) {
        let initial = tokio::time::Instant::now() + initial_timeout;
        let mut idle_since = None;
        loop {
            let now = tokio::time::Instant::now();
            if !self.accepted_connection.load(Ordering::SeqCst) {
                if now >= initial {
                    return;
                }
            } else if self.connections.load(Ordering::SeqCst) == 0
                && self.requests.is_empty()
                && self.executions.active_count() == 0
                && self.shells.active_count() == 0
            {
                if now.duration_since(*idle_since.get_or_insert(now)) >= idle_grace {
                    return;
                }
            } else {
                idle_since = None;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// The caller must enforce local-owner OS access before entering here.
    /// Remote transports use authenticated authority, never this entry point.
    pub async fn local_owner_connection(
        self: Arc<Self>,
        reader: impl MessageReader,
        writer: impl MessageWriter,
    ) -> Result<(), HostError> {
        self.authorized_connection(
            reader,
            writer,
            authority::Authority::LocalOwner,
            CancellationToken::new(),
        )
        .await
    }

    fn lifecycle(&self) -> Lifecycle {
        if self.draining.is_cancelled()
            || *self
                .handshake_gate
                .lock()
                .unwrap_or_else(|e| e.into_inner())
        {
            Lifecycle::Draining
        } else {
            Lifecycle::Ready
        }
    }
}

struct ConnectionCount<'a>(&'a AtomicUsize);
impl Drop for ConnectionCount<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

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

use clap::{Args, Parser, Subcommand};
use maka_event_log::{
    EventLog,
    root::{RootNamespaces, initialize},
};
use maka_runtime_host::server::HostError;
use std::{net::SocketAddr, path::PathBuf};

use crate::{candidate, code, serve};

#[derive(Parser)]
#[command(name = "maka", version, about = "Maka")]
pub(super) struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage the native runtime host.
    #[command(subcommand)]
    Host(HostCommand),
    /// Execute a journaled Code Mode cell read from stdin.
    Code(Log),
    /// Inspect the committed execution log.
    Inspect(Log),
}

#[derive(Subcommand)]
enum HostCommand {
    /// Prepare one-time remote Desktop pairing or revoke a credential.
    #[command(subcommand)]
    Access(crate::access::Access),
    /// Initialize an empty native State Root, or verify its existing identity.
    Init(Root),
    /// Install this native executable as the root's managed Host.
    Install(crate::deployment::Install),
    /// Activate an installed Host and report its verified loopback endpoint.
    Activate(crate::deployment::Activate),
    /// Connect an installed Host to stdin/stdout using the client wire protocol.
    Connect(crate::deployment::Connect),
    /// Update code and optional deployment configuration at a safe boundary.
    Update(crate::deployment::Update),
    /// Finish an interrupted deployment update without choosing another target.
    Reconcile(crate::deployment::Expected),
    /// Stop a deployment without changing its configuration or pending update.
    Stop(crate::deployment::Control),
    /// Restart the current deployment without applying a pending update.
    Restart(crate::deployment::Control),
    /// Revoke startup and unregister its service, retaining State Root data.
    Uninstall(crate::deployment::Control),
    /// Observe a Host or managed deployment without starting or changing it.
    Status(crate::deployment::Status),
    /// Read a bounded tail of supervised Host diagnostics without starting it.
    Logs(crate::deployment::Logs),
    /// Retire the exact current Host, preserving work at safe step boundaries.
    Retire {
        #[command(flatten)]
        root: Root,
        #[arg(long)]
        expected_host_epoch: Option<String>,
        /// Coordinate with this exact connected client without interrupting other clients.
        #[arg(long, requires = "expected_host_epoch")]
        handoff_connection_id: Option<uuid::Uuid>,
        /// Permit interrupting other clients and non-cooperative resources.
        #[arg(long)]
        allow_interrupt_active_tasks: bool,
    },
    /// Run a discoverable ephemeral Host under its launcher.
    Candidate(candidate::Candidate),
    /// Run only the currently admitted supervised deployment.
    #[command(hide = true)]
    ServiceRun(crate::deployment::ServiceRun),
    /// Serve a native State Root over a private local endpoint.
    Serve {
        #[command(flatten)]
        root: Root,
        #[arg(long)]
        websocket: Option<SocketAddr>,
    },
}

#[derive(Args)]
pub(super) struct Root {
    #[arg(long, value_name = "DIRECTORY")]
    pub(super) root: PathBuf,
}

#[derive(Args)]
struct Log {
    #[arg(long, value_name = "FILE")]
    log: PathBuf,
}

impl Cli {
    pub(super) fn error_exit_code(&self) -> u8 {
        if matches!(self.command, Command::Host(HostCommand::Candidate(_))) {
            70
        } else {
            1
        }
    }

    pub(super) async fn run(self) -> Result<(), HostError> {
        match self.command {
            Command::Host(HostCommand::Access(args)) => args.run().await,
            Command::Host(HostCommand::Candidate(args)) => args.run().await,
            Command::Host(HostCommand::Install(args)) => args.run().await,
            Command::Host(HostCommand::Activate(args)) => args.run().await,
            Command::Host(HostCommand::Connect(args)) => args.run().await,
            Command::Host(HostCommand::Update(args)) => args.run().await,
            Command::Host(HostCommand::Reconcile(args)) => args.reconcile().await,
            Command::Host(HostCommand::Stop(args)) => {
                args.run(crate::deployment::ControlAction::Stop).await
            }
            Command::Host(HostCommand::Restart(args)) => {
                args.run(crate::deployment::ControlAction::Restart).await
            }
            Command::Host(HostCommand::Uninstall(args)) => {
                args.run(crate::deployment::ControlAction::Uninstall).await
            }
            Command::Host(HostCommand::Init(args)) => {
                let root_id = initialize(&args.root, &RootNamespaces::for_current_account()?)?;
                println!("{}", serde_json::json!({"rootId": root_id}));
                Ok(())
            }
            Command::Host(HostCommand::Serve { root, websocket }) => {
                #[cfg(windows)]
                crate::windows::own_process_tree()?;
                serve::run(&root.root, websocket, None).await
            }
            Command::Host(HostCommand::ServiceRun(args)) => args.run().await,
            Command::Host(HostCommand::Status(args)) => args.run().await,
            Command::Host(HostCommand::Logs(args)) => args.run().await,
            Command::Host(HostCommand::Retire {
                root,
                expected_host_epoch,
                handoff_connection_id,
                allow_interrupt_active_tasks,
            }) => {
                let mut client = crate::host_client::HostClient::connect(&root.root, None).await?;
                let result = client
                    .retire(
                        expected_host_epoch.as_deref(),
                        allow_interrupt_active_tasks,
                        handoff_connection_id,
                    )
                    .await?;
                drop(client);
                println!("{}", serde_json::to_string(&result)?);
                Ok(())
            }
            Command::Code(args) => code::run(&args.log).await,
            Command::Inspect(args) => {
                let log = EventLog::open(&args.log).await?;
                let prefix = log.prefix(10_000, 8 * 1024 * 1024).await?;
                println!("{}", serde_json::to_string(&prefix)?);
                log.shutdown().await?;
                Ok(())
            }
        }
    }
}

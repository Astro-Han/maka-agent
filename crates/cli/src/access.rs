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

use crate::{args::Root, host_client::HostClient};
use clap::{Args, Subcommand};
use maka_event_log::root::RootNamespaces;
use maka_protocol::{
    Operation,
    access::{
        decode_issue_result, decode_prepare_input, decode_revoke_input, decode_revoke_result,
    },
};
use maka_runtime::access::{
    AccessCredentialPrepareInput, AccessCredentialRevokeInput, ManagedPrincipalKind,
};
use maka_runtime_host::{access_delivery, server::HostError};
use serde::Serialize;

#[derive(Subcommand)]
pub(super) enum Access {
    /// Print short-lived remote Desktop pairing JSON, including its secret credential.
    Prepare(Prepare),
    /// Revoke a credential and disconnect its remote clients.
    Revoke(Revoke),
}

#[derive(Args)]
pub(super) struct Prepare {
    #[command(flatten)]
    root: Root,
    #[arg(long)]
    principal: String,
}

#[derive(Args)]
pub(super) struct Revoke {
    #[command(flatten)]
    root: Root,
    #[arg(long)]
    credential_id: String,
}

// No Debug: this receipt is intended only for the importing Client.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Pairing {
    root_id: String,
    credential_id: String,
    credential: String,
}

impl Access {
    pub async fn run(self) -> Result<(), HostError> {
        match self {
            Self::Prepare(args) => args.run().await,
            Self::Revoke(args) => args.run().await,
        }
    }
}

impl Prepare {
    async fn run(self) -> Result<(), HostError> {
        // Match the Desktop owner policy, without granting arbitrary Host paths.
        // Pending credentials expire and must be finalized by the importing
        // Client; its instance identity then remains bound to the credential.
        let input = serde_json::to_value(AccessCredentialPrepareInput {
            principal_kind: ManagedPrincipalKind::RemoteOwner,
            principal_id: self.principal,
            operation_grants: Operation::ALL
                .iter()
                .filter(|operation| operation.allows_remote_owner())
                .map(|operation| operation.as_str().into())
                .collect(),
            can_publish_client_capabilities: true,
            can_use_host_paths: false,
            bind_client_instance: Some(true),
        })?;
        decode_prepare_input(&input)?;
        let mut client = HostClient::connect(&self.root.root, None).await?;
        let result = decode_issue_result(
            &client
                .request(Operation::AccessCredentialPrepare, input)
                .await?,
        )?;
        let control = RootNamespaces::for_current_account()?
            .control
            .join(client.root_id());
        let credential_id = result.credential_id.clone();
        let credential = tokio::task::spawn_blocking(move || {
            access_delivery::consume(&control, &result.delivery_id, &credential_id)
        })
        .await??;
        println!(
            "{}",
            serde_json::to_string(&Pairing {
                root_id: client.root_id().into(),
                credential_id: result.credential_id,
                credential,
            })?
        );
        // Retain the connection through delivery consumption and output.
        drop(client);
        Ok(())
    }
}

impl Revoke {
    async fn run(self) -> Result<(), HostError> {
        let input = serde_json::to_value(AccessCredentialRevokeInput {
            credential_id: self.credential_id,
        })?;
        decode_revoke_input(&input)?;
        let mut client = HostClient::connect(&self.root.root, None).await?;
        let result = decode_revoke_result(
            &client
                .request(Operation::AccessCredentialRevoke, input)
                .await?,
        )?;
        println!("{}", serde_json::to_string(&result)?);
        Ok(())
    }
}

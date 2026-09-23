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

use crate::app::App;
use maka_protocol::connection_effects::{
    ConnectionEffectFailureClass as Failure, ConnectionEffectRejectionReason as Rejected,
    ConnectionModelFetchResult as Result,
};

pub(super) fn failure(result: &Result) -> (&'static str, bool) {
    match result {
        Result::Committed { .. } => unreachable!("committed fetch closes the dialog"),
        Result::Superseded { .. } => ("connection-models-fetch-changed", true),
        Result::Rejected { reason } => (
            match reason {
                Rejected::ConnectionNotFound | Rejected::ConnectionDisabled => {
                    "connection-models-fetch-unavailable"
                }
                Rejected::ProviderActionUnavailable => "connection-models-fetch-unsupported",
                Rejected::CredentialNotConfigured => "connection-models-fetch-key",
            },
            true,
        ),
        Result::Failed { error_class } => (
            match error_class {
                Failure::Auth => "connection-models-fetch-auth",
                Failure::InvalidResponse => "connection-models-fetch-invalid",
                Failure::Timeout
                | Failure::ProviderUnavailable
                | Failure::Network
                | Failure::Unknown => "connection-models-fetch-failed",
            },
            false,
        ),
    }
}

impl App {
    pub(super) fn connection_models_acknowledged(&mut self, result: &Result) {
        let Result::Committed { connection, .. } = result else {
            return;
        };
        // Discovery can enable a first model. The response has no model IDs, so
        // invalidate older rows instead of fabricating a complete editable basis.
        // A newer authoritative projection must survive a late acknowledgement.
        self.connections.rows.retain(|row| {
            row.id != connection.connection_id || row.revision >= connection.revision
        });
    }
}

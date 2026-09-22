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

use super::{Host, HostError};
use maka_protocol::{Operation, OperationError, OperationErrorCode as Code, Outcome};

pub(super) async fn execute(host: &Host, operation: Operation) -> Result<Outcome, HostError> {
    #[cfg(not(windows))]
    {
        use maka_protocol::sandbox_setup::Status;
        let _ = host;
        if operation == Operation::SandboxSetupQuery {
            return Ok(Outcome::success(serde_json::to_value(Status::NotRequired)?));
        }
        Ok(Outcome::failure(OperationError {
            code: Code::OperationUnavailable,
            message: "This Host does not use Windows sandbox provisioning".into(),
        }))
    }
    #[cfg(windows)]
    {
        use crate::sandbox::windows::{Installation, Operation as ProvisionOperation, Provision};
        use std::{io, time::Duration};
        let root = host.root.canonical_path().to_owned();
        if operation != Operation::SandboxSetupQuery {
            let provision = Provision {
                root: root.clone(),
                operation: match operation {
                    Operation::SandboxSetupInstall => ProvisionOperation::Setup,
                    Operation::SandboxSetupRemove => ProvisionOperation::Remove,
                    _ => unreachable!("sandbox setup dispatch"),
                },
            };
            let executable = std::env::current_exe()?;
            // Consent does not hold execution admission or block other requests.
            // Closing observation before delivery prevents late admission;
            // after delivery the one-shot helper owns its durable completion.
            let result = tokio::select! {
                result = provision.request(&executable) => result,
                _ = host.draining.cancelled() => Err(io::Error::new(io::ErrorKind::Interrupted,
                    "Host stopped observing sandbox setup; query setup status before retrying")),
                _ = tokio::time::sleep(Duration::from_secs(180)) => Err(io::Error::new(io::ErrorKind::TimedOut,
                    "Sandbox setup observation timed out; query setup status before retrying")),
            };
            if let Err(error) = result {
                // ShellExecute reports a rejected UAC prompt before the helper
                // starts. This is a known non-admission, not an unknown outcome.
                if error.raw_os_error()
                    == Some(windows_sys::Win32::Foundation::ERROR_CANCELLED as i32)
                {
                    return Ok(Outcome::failure(OperationError {
                        code: Code::UserCancelled,
                        message: "Sandbox setup was cancelled".into(),
                    }));
                }
                return Ok(Outcome::failure(OperationError {
                    code: match error.kind() {
                        io::ErrorKind::TimedOut | io::ErrorKind::Interrupted => {
                            Code::OutcomeUnknown
                        }
                        io::ErrorKind::PermissionDenied => Code::Unauthorized,
                        _ => Code::OperationConflict,
                    },
                    message: error.to_string(),
                }));
            }
        }
        match tokio::task::spawn_blocking(move || Installation::new(&root).status()).await? {
            Ok(status) => Ok(Outcome::success(serde_json::to_value(status)?)),
            Err(error) => Ok(Outcome::failure(OperationError {
                code: Code::InternalFailure,
                message: format!("Cannot inspect Windows sandbox installation: {error}"),
            })),
        }
    }
}

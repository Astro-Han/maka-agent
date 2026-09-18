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

use super::{
    CommandProjection, EntryProjection, ExecutorProjection, Failure, PackageProjection,
    ToolProjection,
};
use crate::{Operation, ProtocolError, Result, codec};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    New,
    Recovering,
    Ready,
    Degraded,
    Fenced,
    Draining,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Convergence {
    Unknown,
    Converged,
    Diverged,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cleanup {
    Complete,
    Pending,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Durability {
    Committed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Status {
    pub phase: Phase,
    pub authority_epoch: u64,
    pub convergence: Convergence,
    pub installed_package_count: usize,
    pub layered_package_count: usize,
    pub desired_entry_count: usize,
    pub live_entry_count: usize,
    pub failure_count: usize,
    pub fence_diagnostic: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub authority_epoch: u64,
    pub durability: Durability,
    pub convergence: Convergence,
    pub cleanup: Cleanup,
    pub failures: Vec<Failure>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Installed {
    #[serde(flatten)]
    pub receipt: Receipt,
    pub extension_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Exported {
    pub target_path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "view", rename_all = "snake_case")]
pub enum QueryResult {
    Status(Status),
    Packages(Page<PackageProjection>),
    Entries(Page<EntryProjection>),
    Tools(Page<ToolProjection>),
    Commands(Page<CommandProjection>),
    Executors(Page<ExecutorProjection>),
    Failures(Page<Failure>),
}

pub fn decode_output(operation: Operation, value: &Value) -> Result<Value> {
    if serde_json::to_vec(value)
        .map_err(|error| ProtocolError::invalid(error.to_string()))?
        .len()
        > 128 * 1024
    {
        return Err(ProtocolError::invalid("plugin output exceeds 128 KiB"));
    }
    match operation {
        Operation::PluginRemote => {
            super::validate_remote_result(value)?;
        }
        Operation::PluginClientQuery => super::client::validate_output(value)?,
        Operation::PluginPlatformQuery => {
            let row = codec::record(value, "plugin query result")?;
            if value["view"] == "status" {
                codec::exact(
                    row,
                    &[
                        "view",
                        "phase",
                        "authorityEpoch",
                        "convergence",
                        "installedPackageCount",
                        "layeredPackageCount",
                        "desiredEntryCount",
                        "liveEntryCount",
                        "failureCount",
                        "fenceDiagnostic",
                    ],
                )?;
            } else {
                codec::exact(row, &["view", "items", "nextCursor"])?;
                if !value["nextCursor"].is_null() {
                    codec::string(&value["nextCursor"], "plugin cursor", 4096)?;
                }
            }
            let output: QueryResult = decode(value)?;
            let count = match &output {
                QueryResult::Status(_) => {
                    codec::count(&value["authorityEpoch"], "plugin authority epoch")?;
                    0
                }
                QueryResult::Packages(page) => page.items.len(),
                QueryResult::Entries(page) => page.items.len(),
                QueryResult::Tools(page) => page.items.len(),
                QueryResult::Commands(page) => page.items.len(),
                QueryResult::Executors(page) => page.items.len(),
                QueryResult::Failures(page) => page.items.len(),
            };
            if count > 64 {
                return Err(ProtocolError::invalid("plugin page exceeds 64 items"));
            }
        }
        Operation::PluginPackageInstall => {
            let installed: Installed = decode(value)?;
            maka_plugins::identifier(&installed.extension_id)
                .map_err(|error| ProtocolError::invalid(error.to_string()))?;
            validate_receipt(value, &installed.receipt)?;
        }
        Operation::PluginPackageExport => {
            let _: Exported = decode(value)?;
        }
        Operation::PluginCompositionApply
        | Operation::PluginPackageReload
        | Operation::PluginPackageUninstall
        | Operation::PluginPlatformReconcile => {
            let receipt: Receipt = decode(value)?;
            validate_receipt(value, &receipt)?;
        }
        _ => return Err(ProtocolError::invalid("not a plugin operation")),
    }
    Ok(value.clone())
}

fn validate_receipt(value: &Value, receipt: &Receipt) -> Result<()> {
    codec::shaped(
        codec::record(value, "plugin receipt")?,
        &[
            "authorityEpoch",
            "durability",
            "convergence",
            "cleanup",
            "failures",
        ],
        &["extensionId"],
    )?;
    codec::count(&value["authorityEpoch"], "plugin authority epoch")?;
    if receipt.convergence == Convergence::Unknown || receipt.failures.len() > 64 {
        return Err(ProtocolError::invalid("invalid plugin mutation receipt"));
    }
    Ok(())
}

fn decode<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T> {
    serde_json::from_value(value.clone()).map_err(|error| ProtocolError::invalid(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plugin_wire_retains_committed_failure_and_rejects_incomplete_pages() {
        let installed = Installed {
            extension_id: "example".into(),
            receipt: Receipt {
                authority_epoch: 2,
                durability: Durability::Committed,
                convergence: Convergence::Diverged,
                cleanup: Cleanup::Pending,
                failures: vec![Failure {
                    entry_id: Some("example".into()),
                    extension_id: Some("example".into()),
                    diagnostic: "activation failed".into(),
                }],
            },
        };
        decode_output(
            Operation::PluginPackageInstall,
            &serde_json::to_value(installed).unwrap(),
        )
        .unwrap();
        let tool = QueryResult::Tools(Page {
            next_cursor: None,
            items: vec![ToolProjection {
                identity: super::super::ContributionIdentity {
                    entry_id: "example".into(),
                    scope_id: maka_plugins::composition::Scope::Profile,
                    extension_id: "example".into(),
                    generation: 1,
                },
                tool_name: "example".into(),
                active_calls: 0,
                retired: false,
            }],
        });
        let mut page = serde_json::to_value(tool).unwrap();
        decode_output(Operation::PluginPlatformQuery, &page).unwrap();
        page.as_object_mut().unwrap().remove("nextCursor");
        assert!(decode_output(Operation::PluginPlatformQuery, &page).is_err());
        assert!(
            super::super::decode_input(
                Operation::PluginPlatformQuery,
                &json!({"view":"status","limit":1})
            )
            .is_err()
        );
        assert!(super::super::decode_input(Operation::PluginCompositionApply, &json!({"baseGeneration":1,"operations":[{"type":"move","entryId":"example","position":-1}]})).is_err());
    }
}

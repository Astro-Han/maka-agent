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

use crate::{Operation, ProtocolError, Result, codec};
use maka_plugins::composition::{Operation as CompositionOperation, Scope};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum View {
    Status,
    Packages,
    Entries,
    Tools,
    Commands,
    Executors,
    Failures,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Query {
    pub view: View,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_id: Option<Scope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Apply {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_generation: Option<u64>,
    pub operations: Vec<CompositionOperation>,
}

pub enum Input {
    Authorization(Box<super::AuthorizationInput>),
    Remote(Box<super::RemoteRequest>),
    Client(super::ClientQuery),
    Query(Query),
    Apply(Apply),
    Install {
        source_path: String,
    },
    Uninstall {
        extension_id: String,
    },
    Reload {
        extension_id: String,
    },
    Export {
        extension_id: String,
        target_path: String,
    },
    Reconcile,
}

pub fn decode_input(operation: Operation, value: &Value) -> Result<Input> {
    let row = codec::record(value, "plugin input")?;
    Ok(match operation {
        Operation::PluginAuthorization => {
            let request: super::AuthorizationInput = decode(value.clone())?;
            request.validate()?;
            Input::Authorization(Box::new(request))
        }
        Operation::PluginRemote => {
            let request: super::RemoteRequest = decode(value.clone())?;
            request.validate()?;
            Input::Remote(Box::new(request))
        }
        Operation::PluginClientQuery => {
            let query: super::ClientQuery = decode(value.clone())?;
            query.validate()?;
            Input::Client(query)
        }
        Operation::PluginPlatformQuery => {
            codec::shaped(row, &["view"], &["rootId", "cursor", "limit"])?;
            let mut normalized = value.clone();
            if let Some(limit) = row.get("limit") {
                let limit = codec::count(limit, "plugin page size")?;
                if !(1..=64).contains(&limit) {
                    return Err(invalid("plugin page size must be 1..=64"));
                }
                normalized["limit"] = limit.into();
            }
            if let Some(cursor) = row.get("cursor") {
                codec::string(cursor, "plugin cursor", 4096)?;
            }
            if let Some(root) = row.get("rootId") {
                codec::string(root, "plugin root", 256)?;
            }
            let query: Query = decode(normalized)?;
            if query.view == View::Status
                && (query.root_id.is_some() || query.cursor.is_some() || query.limit.is_some())
            {
                return Err(invalid("status does not accept root or pagination"));
            }
            if query.root_id.is_some()
                && !matches!(
                    query.view,
                    View::Entries | View::Tools | View::Commands | View::Executors
                )
            {
                return Err(invalid("this plugin view does not accept a root"));
            }
            Input::Query(query)
        }
        Operation::PluginCompositionApply => {
            codec::shaped(row, &["operations"], &["baseGeneration"])?;
            let operations = value["operations"]
                .as_array()
                .filter(|operations| operations.len() <= 4096)
                .ok_or_else(|| invalid("composition requires at most 4096 operations"))?;
            let mut normalized = value.clone();
            if let Some(generation) = row.get("baseGeneration") {
                normalized["baseGeneration"] =
                    codec::count(generation, "composition generation")?.into();
            }
            for (index, operation) in operations.iter().enumerate() {
                if let Some(position) = operation.get("position") {
                    normalized["operations"][index]["position"] =
                        codec::count(position, "entry position")?.into();
                }
            }
            let apply: Apply = decode(normalized)?;
            for operation in &apply.operations {
                operation
                    .validate()
                    .map_err(|error| invalid(error.to_string()))?;
            }
            Input::Apply(apply)
        }
        Operation::PluginPackageInstall => {
            codec::exact(row, &["sourcePath"])?;
            Input::Install {
                source_path: codec::string(&value["sourcePath"], "package source path", 4096)?,
            }
        }
        Operation::PluginPackageUninstall | Operation::PluginPackageReload => {
            codec::exact(row, &["extensionId"])?;
            let extension_id = codec::string(&value["extensionId"], "package identity", 128)?;
            maka_plugins::identifier(&extension_id).map_err(|error| invalid(error.to_string()))?;
            if operation == Operation::PluginPackageReload {
                Input::Reload { extension_id }
            } else {
                Input::Uninstall { extension_id }
            }
        }
        Operation::PluginPackageExport => {
            codec::exact(row, &["extensionId", "targetPath"])?;
            let extension_id = codec::string(&value["extensionId"], "package identity", 128)?;
            maka_plugins::identifier(&extension_id).map_err(|error| invalid(error.to_string()))?;
            Input::Export {
                extension_id,
                target_path: codec::string(&value["targetPath"], "bundle destination", 4096)?,
            }
        }
        Operation::PluginPlatformReconcile => {
            codec::exact(row, &[])?;
            Input::Reconcile
        }
        _ => return Err(invalid("not a plugin operation")),
    })
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|error| invalid(error.to_string()))
}
fn invalid(message: impl Into<String>) -> ProtocolError {
    ProtocolError::invalid(message)
}

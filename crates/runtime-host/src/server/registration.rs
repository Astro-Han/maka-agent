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
use maka_protocol::{COMPATIBILITY_EPOCH, COMPOSITION_ID};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

/// Owns only this epoch's discovery record, while the Host retains both root leases.
pub struct Registration {
    host: Arc<Host>,
    path: PathBuf,
}

impl Host {
    pub fn publish_registration(
        self: &Arc<Self>,
        endpoint: &Path,
        websocket: Option<std::net::SocketAddr>,
    ) -> Result<Registration, HostError> {
        self.root.validate_current()?;
        let endpoint = endpoint.to_str().ok_or("endpoint is not UTF-8")?;
        if endpoint.encode_utf16().count() > 512 {
            return Err("endpoint exceeds discovery bound".into());
        }
        let mut value = json!({
            "kind":"maka-runtime-host", "schemaVersion":1, "rootId":self.root_id(),
            "hostEpoch":self.epoch, "endpoint":endpoint, "protocolMin":0, "protocolMax":0,
            "compatibilityEpoch":COMPATIBILITY_EPOCH, "compositionId":COMPOSITION_ID,
            "compositionRevision":"3", "lifecycleMode":"ephemeral", "state":"ready",
            "pid":std::process::id(),
            "createdAt":time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)?,
        });
        if let Some(generation) = &self.options.generation {
            value["generation"] = generation.clone().into();
        }
        if let Some(address) = websocket {
            value["websocketEndpoints"] = json!([format!("ws://{address}/runtime-host")]);
        }
        let path = self.control_directory().join("registration.json");
        let mut temporary = tempfile::NamedTempFile::new_in(self.control_directory())?;
        serde_json::to_writer(&mut temporary, &value)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        self.root.validate_current()?;
        temporary.persist(&path)?;
        Ok(Registration {
            host: self.clone(),
            path,
        })
    }
}

impl Registration {
    pub fn remove(self) -> Result<(), HostError> {
        self.remove_current()
    }

    fn remove_current(&self) -> Result<(), HostError> {
        self.host.root.validate_current()?;
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file() || metadata.len() > 16 * 1024 {
            return Err("invalid discovery record".into());
        }
        let mut bytes = Vec::new();
        fs::File::open(&self.path)?
            .take(16 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        let current: Value = serde_json::from_slice(&bytes)?;
        if current["hostEpoch"].as_str() == Some(&self.host.epoch) {
            fs::remove_file(&self.path)?;
        }
        Ok(())
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Err(error) = self.remove_current() {
            eprintln!("could not remove Host registration: {error}");
        }
    }
}

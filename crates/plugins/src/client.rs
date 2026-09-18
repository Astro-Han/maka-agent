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

//! Published client bytes are immutable and owned by the activation, not a path.
use crate::{Error, package::Package};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub const SDK_VERSION: u32 = 1;

#[derive(Debug)]
pub struct Bundle {
    pub package_id: String,
    pub content_digest: String,
    pub client_digest: String,
    pub sdk_version: u32,
    pub dependencies: Vec<String>,
    source: Arc<str>,
}
impl Bundle {
    /// Statically linked plugins carry their client bytes in the same Host build.
    pub fn builtin(
        package_id: &str,
        release: &str,
        source: &'static str,
    ) -> Result<Arc<Self>, Error> {
        crate::identifier(package_id)?;
        if source.len() > crate::package::MAX_FILE_BYTES {
            return Err(Error::Invalid(
                "built-in Client bundle exceeds 8 MiB".into(),
            ));
        }
        let client_digest = format!("sha256-{:x}", Sha256::digest(source.as_bytes()));
        let identity = serde_json::to_vec(&(package_id, release, &client_digest))
            .map_err(|error| Error::Invalid(error.to_string()))?;
        Ok(Arc::new(Self {
            package_id: package_id.into(),
            content_digest: format!("sha256-{:x}", Sha256::digest(identity)),
            client_digest,
            sdk_version: SDK_VERSION,
            dependencies: Vec::new(),
            source: source.into(),
        }))
    }
    /// Validate/hash once at load, never while servicing a bundle chunk.
    pub fn from_package(package: &Package) -> Result<Option<Arc<Self>>, Error> {
        let Some(entry) = &package.manifest().client else {
            return Ok(None);
        };
        if package.manifest().dependencies.len() > 128 {
            return Err(Error::Invalid(
                "client bundle exceeds 128 dependencies".into(),
            ));
        }
        let bytes = package
            .file(&entry.entry)
            .ok_or_else(|| Error::Invalid("client entrypoint bytes are missing".into()))?;
        let source = std::str::from_utf8(bytes)
            .map_err(|_| Error::Invalid("client entrypoint is not UTF-8".into()))?;
        Ok(Some(Arc::new(Self {
            package_id: package.manifest().id.clone(),
            content_digest: package.digest().into(),
            client_digest: format!("sha256-{:x}", Sha256::digest(bytes)),
            sdk_version: entry.sdk_version,
            dependencies: package
                .manifest()
                .dependencies
                .iter()
                .map(|item| item.id.clone())
                .collect(),
            source: source.into(),
        })))
    }
    pub fn source(&self) -> &str {
        &self.source
    }
}

#[derive(Clone)]
pub struct Client {
    pub bundle: Arc<Bundle>,
    pub config: Value,
}

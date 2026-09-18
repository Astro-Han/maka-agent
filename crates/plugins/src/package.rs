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

mod bundle;
mod io;
mod manifest;
mod path;
mod yaml;
pub use io::PackageIoError;
pub use manifest::{
    ClientEntrypoint, CompositionFile, Dependency, HostEntrypoint, Manifest, VmMode,
};
pub use path::validate_path;

use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use crate::Error;

pub const MAX_FILES: usize = 256;
pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_PACKAGE_BYTES: usize = 16 * 1024 * 1024;
pub const MANIFEST_FILE: &str = "maka.extension.json";

/// Validated immutable bytes. Retained instances keep their exact package alive.
#[derive(Clone, Debug)]
pub struct Package {
    manifest: Arc<Manifest>,
    files: Arc<BTreeMap<String, Vec<u8>>>,
    digest: Arc<str>,
}

impl Package {
    pub fn composition(&self) -> Result<Vec<crate::composition::Operation>, Error> {
        let Some(composition) = &self.manifest.composition else {
            return Ok(Vec::new());
        };
        let bytes = self
            .file(&composition.patch)
            .expect("validated composition entry");
        let value = yaml::parse(bytes)?;
        let operations: Vec<crate::composition::Operation> = serde_json::from_value(value)
            .map_err(|error| Error::Invalid(format!("composition patch: {error}")))?;
        if operations.len() > 4096 {
            return Err(Error::Invalid(
                "composition patch exceeds 4096 operations".into(),
            ));
        }
        Ok(operations)
    }

    pub fn new(files: BTreeMap<String, Vec<u8>>) -> Result<Self, Error> {
        if files.is_empty() || files.len() > MAX_FILES {
            return Err(Error::Invalid("package must contain 1..=256 files".into()));
        }
        let mut total = 0;
        let mut paths = BTreeSet::new();
        for (path, content) in &files {
            validate_path(path)?;
            if !paths.insert(path.to_lowercase()) {
                return Err(Error::Invalid("package has case-colliding paths".into()));
            }
            total += content.len();
            if content.len() > MAX_FILE_BYTES || total > MAX_PACKAGE_BYTES {
                return Err(Error::Invalid("package exceeds byte limits".into()));
            }
        }
        for path in &paths {
            for (index, _) in path.match_indices('/') {
                if paths.contains(&path[..index]) {
                    return Err(Error::Invalid("package file is also a directory".into()));
                }
            }
        }
        let bytes = files
            .get(MANIFEST_FILE)
            .ok_or_else(|| Error::Invalid("package manifest is missing".into()))?;
        if bytes.len() > 256 * 1024 {
            return Err(Error::Invalid("manifest exceeds 256 KiB".into()));
        }
        let manifest: Manifest = serde_json::from_slice(bytes)
            .map_err(|error| Error::Invalid(format!("manifest: {error}")))?;
        manifest.validate()?;
        for entry in manifest
            .runtime
            .iter()
            .map(|entry| &entry.entry)
            .chain(manifest.client.iter().map(|entry| &entry.entry))
            .chain(manifest.composition.iter().map(|entry| &entry.patch))
        {
            if !files.contains_key(entry) {
                return Err(Error::Invalid(format!("package entry is missing: {entry}")));
            }
        }
        let digest = content_digest(&files);
        Ok(Self {
            manifest: Arc::new(manifest),
            files: Arc::new(files),
            digest: digest.into(),
        })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.files
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub fn file(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }
}

fn content_digest(files: &BTreeMap<String, Vec<u8>>) -> String {
    // The existing bundle format sorts paths as JavaScript strings (UTF-16).
    let mut files: Vec<_> = files.iter().collect();
    files.sort_unstable_by(|(left, _), (right, _)| left.encode_utf16().cmp(right.encode_utf16()));
    let mut hash = Sha256::new();
    for (path, content) in files {
        hash.update((path.len() as u64).to_be_bytes());
        hash.update(path.as_bytes());
        hash.update((content.len() as u64).to_be_bytes());
        hash.update(content);
    }
    format!("sha256-{:x}", hash.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn package_pins_exact_bytes_and_rejects_cross_platform_path_aliases() {
        let manifest = serde_json::to_vec(&json!({
            "schemaVersion":1,"id":"graph","runtime":{"entry":"index.mjs","sdkVersion":1,"vm":"dedicated"}
        })).unwrap();
        let files = BTreeMap::from([
            (MANIFEST_FILE.into(), manifest),
            ("index.mjs".into(), b"export default {}".to_vec()),
        ]);
        let package = Package::new(files.clone()).unwrap();
        let bundle = package.to_bundle();
        assert_eq!(
            Package::from_bundle(&bundle).unwrap().digest(),
            package.digest()
        );
        let mut corrupt: serde_json::Value = serde_json::from_slice(&bundle).unwrap();
        corrupt["files"][0]["content"] = json!("Y29ycnVwdA==");
        assert!(Package::from_bundle(&serde_json::to_vec(&corrupt).unwrap()).is_err());
        assert_eq!(
            package.manifest().runtime.as_ref().unwrap().vm,
            VmMode::Dedicated
        );
        let mut updated = files.clone();
        updated.insert(
            "index.mjs".into(),
            b"export default { updated: true }".to_vec(),
        );
        assert_ne!(package.digest(), Package::new(updated).unwrap().digest());
        assert_eq!(package.file("index.mjs").unwrap(), b"export default {}");
        for path in [
            "../escape",
            "INDEX.MJS",
            "index.mjs/child",
            "dir/CON.txt",
            "C:payload",
            "trailing.",
        ] {
            let mut invalid = files.clone();
            invalid.insert(path.into(), vec![]);
            assert!(Package::new(invalid).is_err(), "{path}");
        }
    }
}

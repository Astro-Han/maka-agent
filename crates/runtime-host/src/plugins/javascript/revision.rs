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

use maka_plugins::{
    Error,
    revision::{Basis, Invalidation, Revision},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Mutex, time::Duration};

#[derive(Default)]
pub(super) struct Revisions {
    values: Mutex<BTreeMap<String, Revision>>,
    writes: Mutex<BTreeMap<String, Invalidation>>,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Request {
    New,
    Capture { handle: String },
    Invalidate { handle: String },
    Release { handle: String },
    Close { handle: String },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Reference {
    handle: String,
    version: u64,
}
impl Revisions {
    fn get(&self, handle: &str) -> Result<Revision, Error> {
        self.values
            .lock()
            .unwrap()
            .get(handle)
            .cloned()
            .ok_or(Error::Retired)
    }
    pub fn basis(&self, reference: Reference) -> Result<Basis, Error> {
        if reference.version >= (1 << 53) {
            return Err(Error::Invalid("invalid preparation version".into()));
        }
        Ok(self.get(&reference.handle)?.at(reference.version))
    }
    pub async fn call(&self, request: Request) -> Result<Value, Error> {
        match request {
            Request::New => {
                let mut values = self.values.lock().unwrap();
                if values.len() >= 256 {
                    return Err(Error::Invalid("preparation revision limit reached".into()));
                }
                let handle = uuid::Uuid::new_v4().to_string();
                values.insert(handle.clone(), Revision::default());
                Ok(Value::String(handle))
            }
            Request::Capture { handle } => {
                let basis =
                    tokio::time::timeout(Duration::from_secs(5), self.get(&handle)?.capture())
                        .await
                        .map_err(|_| {
                            Error::Invalid("preparation update is still in progress".into())
                        })?;
                Ok(json!({"handle":handle, "version":basis.version()}))
            }
            Request::Invalidate { handle } => {
                let revision = self.get(&handle)?;
                let guard = tokio::time::timeout(Duration::from_secs(5), revision.invalidate())
                    .await
                    .map_err(|_| {
                        Error::Invalid("preparation admission is still in progress".into())
                    })?;
                self.get(&handle)?;
                let mut writes = self.writes.lock().unwrap();
                if writes.len() >= 128 {
                    return Err(Error::Invalid("preparation update limit reached".into()));
                }
                let handle = uuid::Uuid::new_v4().to_string();
                writes.insert(handle.clone(), guard);
                Ok(Value::String(handle))
            }
            Request::Release { handle } => {
                self.writes.lock().unwrap().remove(&handle);
                Ok(Value::Null)
            }
            Request::Close { handle } => {
                let Some(revision) = self.values.lock().unwrap().get(&handle).cloned() else {
                    return Ok(Value::Null);
                };
                let _guard = tokio::time::timeout(Duration::from_secs(5), revision.invalidate())
                    .await
                    .map_err(|_| {
                        Error::Invalid("preparation update is still in progress".into())
                    })?;
                self.values.lock().unwrap().remove(&handle);
                Ok(Value::Null)
            }
        }
    }
}

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

//! Graph-owned decisions, atomically committed through namespaced plugin storage.
//! Execution admission and outcomes belong to Host, not this repository.
use crate::{
    Epoch, Error, GraphId, Mode, WorkId,
    control::{Control, EpochPage, Intent, Wake},
    schedule::{CommittedUpdate, Finish, Source, Stop, Update, Work},
    store::Store,
};
use futures_util::future::BoxFuture;
use maka_plugins::storage::{Data, Mutation, Record, Scan, Store as Storage, StoreError};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::sync::Arc;

const MAX_SAFE: u64 = (1 << 53) - 1;

pub struct Repository {
    storage: Arc<dyn Storage>,
    prefix: String,
}

#[derive(Serialize, Deserialize)]
struct Header {
    control: Control,
    work_count: usize,
}
#[derive(Serialize, Deserialize)]
struct Decision {
    graph_id: GraphId,
    source: Source,
    work_ids: Vec<WorkId>,
    stop: Vec<Stop>,
    finish: Option<Finish>,
    revision: u64,
    committed_at: u64,
}
#[derive(Serialize, Deserialize)]
struct WorkRecord {
    work: Work,
    revision: u64,
}
#[derive(Serialize, Deserialize)]
struct DecisionIdentity {
    fingerprint: String,
    revision: u64,
}

impl Repository {
    pub fn new(storage: Arc<dyn Storage>, entry_id: &str) -> Result<Self, Error> {
        crate::identity(entry_id)?;
        Ok(Self {
            storage,
            prefix: format!("graphs/{}/", digest(entry_id)),
        })
    }

    fn key(&self, path: impl std::fmt::Display) -> String {
        format!("{}{path}", self.prefix)
    }

    async fn read<T: DeserializeOwned>(&self, key: &str) -> Result<Option<(u64, T)>, Error> {
        self.storage
            .read(key.to_owned())
            .await?
            .map(decode)
            .transpose()
    }
    async fn required<T: DeserializeOwned>(&self, key: &str) -> Result<(u64, T), Error> {
        self.read(key)
            .await?
            .ok_or_else(|| Error::NotFound(key.into()))
    }
    fn mutation<T: Serialize>(
        &self,
        key: String,
        revision: Option<u64>,
        value: &T,
    ) -> Result<Mutation, Error> {
        Ok(Mutation {
            key,
            expected_revision: revision,
            data: Data::Present(serde_json::to_value(value).map_err(invalid)?),
        })
    }
    async fn commit(&self, mutations: Vec<Mutation>) -> Result<(), Error> {
        match self.storage.batch(mutations).await {
            Ok(_) => Ok(()),
            Err(StoreError::Conflict { .. }) => Err(Error::Conflict),
            Err(error) => Err(error.into()),
        }
    }
    async fn header(&self, graph: &GraphId) -> Result<(u64, Header), Error> {
        self.required(&self.key(format!("headers/{graph}"))).await
    }
    fn write_header(&self, revision: u64, header: &Header) -> Result<Mutation, Error> {
        self.mutation(
            self.key(format!("headers/{}", header.control.epoch.graph_id)),
            Some(revision),
            header,
        )
    }

    pub async fn current(&self, root: &str) -> Result<Option<Control>, Error> {
        crate::identity(root)?;
        let Some((_, graph)) = self
            .read::<GraphId>(&self.key(format!("roots/{}", digest(root))))
            .await?
        else {
            return Ok(None);
        };
        let (_, header) = self.header(&graph).await?;
        if header.control.epoch.root_session_id != root {
            return Err(invalid("Graph root identity changed"));
        }
        Ok(Some(header.control))
    }

    /// Cursor is an opaque storage key. Concurrent roots may appear on later
    /// pages; activation recovery never treats this as a transactional snapshot.
    pub async fn roots(
        &self,
        after: Option<String>,
    ) -> Result<(Vec<String>, Option<String>), Error> {
        let page = self
            .storage
            .scan(Scan {
                prefix: self.key("roots/"),
                after,
            })
            .await?;
        let mut roots = Vec::with_capacity(page.entries.len());
        for entry in page.entries {
            let (_, graph): (_, GraphId) = decode(entry.record)?;
            roots.push(self.header(&graph).await?.1.control.epoch.root_session_id);
        }
        Ok((roots, page.next_after))
    }

    /// Caller proves the previous epoch quiescent. CAS prevents an old owner
    /// replacing a newer epoch; closed epochs never accept fresh decisions.
    pub async fn open(
        &self,
        root: &str,
        mode: Mode,
        previous: Option<&GraphId>,
        now: u64,
    ) -> Result<Epoch, Error> {
        crate::identity(root)?;
        if now > MAX_SAFE {
            return Err(invalid("invalid Graph time"));
        }
        let key = self.key(format!("roots/{}", digest(root)));
        let current = self.read::<GraphId>(&key).await?;
        let (revision, epoch) = match (&current, previous) {
            (None, None) => (None, 1),
            (Some((_, graph)), None) => {
                let control = self.control(root, graph).await?;
                if control.epoch.mode != mode {
                    return Err(Error::Conflict);
                }
                return Ok(control.epoch);
            }
            (Some((revision, graph)), Some(previous)) if graph == previous => {
                let control = self.control(root, graph).await?;
                if !control.closed() {
                    return Err(Error::Closed);
                }
                (
                    Some(*revision),
                    control
                        .epoch
                        .epoch
                        .checked_add(1)
                        .filter(|n| *n <= MAX_SAFE)
                        .ok_or(Error::Conflict)?,
                )
            }
            _ => return Err(Error::Conflict),
        };
        let epoch = Epoch {
            root_session_id: root.into(),
            epoch,
            graph_id: GraphId::new(),
            created_at: now,
            mode,
        };
        let header = Header {
            control: Control {
                epoch: epoch.clone(),
                schedule_revision: 0,
                stop_requested: false,
                stop_target: None,
                finished: false,
            },
            work_count: 0,
        };
        self.commit(vec![
            self.mutation(key, revision, &epoch.graph_id)?,
            self.mutation(
                self.key(format!("headers/{}", epoch.graph_id)),
                None,
                &header,
            )?,
            self.mutation(
                self.key(format!(
                    "epochs/{}/{:016}",
                    digest(root),
                    MAX_SAFE - epoch.epoch
                )),
                None,
                &epoch,
            )?,
        ])
        .await?;
        Ok(epoch)
    }

    pub async fn epochs(&self, root: &str, before: Option<u64>) -> Result<EpochPage, Error> {
        crate::identity(root)?;
        if before.is_some_and(|n| n == 0 || n > MAX_SAFE) {
            return Err(invalid("invalid epoch cursor"));
        }
        let current_epoch = self.current(root).await?.map(|c| c.epoch.epoch);
        let prefix = self.key(format!("epochs/{}/", digest(root)));
        let page = self
            .storage
            .scan(Scan {
                after: before.map(|n| format!("{prefix}{:016}", MAX_SAFE - n)),
                prefix,
            })
            .await?;
        let more = page.next_after.is_some() || page.entries.len() > 32;
        let epochs = page
            .entries
            .into_iter()
            .take(32)
            .map(|entry| decode::<Epoch>(entry.record).map(|(_, epoch)| epoch))
            .collect::<Result<Vec<_>, _>>()?;
        let next_before = more.then(|| epochs.last().map(|e| e.epoch)).flatten();
        Ok(EpochPage {
            epochs,
            current_epoch,
            next_before,
        })
    }

    pub async fn stop(
        &self,
        root: &str,
        graph: &GraphId,
        target: Option<maka_runtime::event::Invocation>,
    ) -> Result<Control, Error> {
        if target
            .as_ref()
            .is_some_and(|target| target.session_id != root)
        {
            return Err(Error::Conflict);
        }
        let (revision, mut header) = self.header(graph).await?;
        let root_key = self.key(format!("roots/{}", digest(root)));
        let (root_revision, current): (_, GraphId) = self.required(&root_key).await?;
        if header.control.epoch.root_session_id != root || current != *graph {
            return Err(Error::Conflict);
        }
        if !header.control.stop_requested {
            header.control.stop_requested = true;
            header.control.stop_target = target;
            self.commit(vec![
                self.write_header(revision, &header)?,
                self.mutation(root_key, Some(root_revision), graph)?,
            ])
            .await?;
        }
        Ok(header.control)
    }

    pub async fn work(
        &self,
        root: &str,
        graph: &GraphId,
        work: &WorkId,
    ) -> Result<Option<Work>, Error> {
        self.control(root, graph).await?;
        Ok(self
            .read::<WorkRecord>(&self.key(format!("work/{graph}/{work}")))
            .await?
            .map(|(_, record)| record.work))
    }

    async fn decision(&self, graph: &GraphId, revision: u64) -> Result<CommittedUpdate, Error> {
        let (_, record): (_, Decision) = self
            .required(&self.key(format!("updates/{graph}/{revision:016}")))
            .await?;
        self.expand(record).await
    }
    async fn expand(&self, record: Decision) -> Result<CommittedUpdate, Error> {
        let mut add_work = Vec::with_capacity(record.work_ids.len());
        for id in &record.work_ids {
            let (_, work): (_, WorkRecord) = self
                .required(&self.key(format!("work/{}/{id}", record.graph_id)))
                .await?;
            if work.revision != record.revision {
                return Err(invalid("Graph work revision changed"));
            }
            add_work.push(work.work);
        }
        Ok(CommittedUpdate {
            update: Update {
                graph_id: record.graph_id,
                source: record.source,
                add_work,
                stop: record.stop,
                finish: record.finish,
            },
            revision: record.revision,
            committed_at: record.committed_at,
        })
    }
}

impl Store for Repository {
    fn control(&self, root: &str, graph: &GraphId) -> BoxFuture<'_, Result<Control, Error>> {
        let (root, graph) = (root.to_owned(), graph.clone());
        Box::pin(async move {
            let (_, header) = self.header(&graph).await?;
            if header.control.epoch.root_session_id != root {
                return Err(Error::Conflict);
            }
            Ok(header.control)
        })
    }
    fn updates(
        &self,
        graph: &GraphId,
        after: u64,
        through: u64,
    ) -> BoxFuture<'_, Result<Vec<CommittedUpdate>, Error>> {
        let graph = graph.clone();
        Box::pin(async move {
            if after > through || through > MAX_SAFE {
                return Err(invalid("invalid schedule cursor"));
            }
            let mut updates = Vec::new();
            let mut bytes = 0;
            for revision in after + 1..=through.min(after.saturating_add(16)) {
                let update = self.decision(&graph, revision).await?;
                let size = serde_json::to_vec(&update).map_err(invalid)?.len();
                if !updates.is_empty() && bytes + size > 2 * 1024 * 1024 {
                    break;
                }
                bytes += size;
                updates.push(update);
            }
            Ok(updates)
        })
    }
    fn commit_update(
        &self,
        update: Update,
        expected: u64,
        now: u64,
    ) -> BoxFuture<'_, Result<CommittedUpdate, Error>> {
        Box::pin(async move {
            update.validate()?;
            if expected >= MAX_SAFE || now > MAX_SAFE {
                return Err(invalid("invalid schedule revision or time"));
            }
            let fingerprint = update.fingerprint()?;
            let identity = self.key(format!(
                "decisions/{}/{}",
                update.graph_id,
                update.identity()
            ));
            if let Some((_, previous)) = self.read::<DecisionIdentity>(&identity).await? {
                if previous.fingerprint != fingerprint {
                    return Err(Error::Conflict);
                }
                return self.decision(&update.graph_id, previous.revision).await;
            }
            let (revision, mut header) = self.header(&update.graph_id).await?;
            if header.control.epoch.root_session_id != update.source.invocation.session_id
                || header.control.schedule_revision != expected
            {
                return Err(Error::Conflict);
            }
            if header.control.closed() {
                return Err(Error::Closed);
            }
            if header.work_count + update.add_work.len() > 1024 {
                return Err(invalid("graph epoch exceeds 1024 work items"));
            }
            header.control.schedule_revision += 1;
            header.control.finished = update.finish.is_some();
            header.work_count += update.add_work.len();
            let record = Decision {
                graph_id: update.graph_id.clone(),
                source: update.source.clone(),
                work_ids: update.add_work.iter().map(|w| w.work_id.clone()).collect(),
                stop: update.stop.clone(),
                finish: update.finish.clone(),
                revision: expected + 1,
                committed_at: now,
            };
            let mut mutations = vec![
                self.write_header(revision, &header)?,
                self.mutation(
                    identity,
                    None,
                    &DecisionIdentity {
                        fingerprint,
                        revision: record.revision,
                    },
                )?,
                self.mutation(
                    self.key(format!(
                        "updates/{}/{:016}",
                        update.graph_id, record.revision
                    )),
                    None,
                    &record,
                )?,
            ];
            for work in &update.add_work {
                mutations.push(self.mutation(
                    self.key(format!("work/{}/{}", update.graph_id, work.work_id)),
                    None,
                    &WorkRecord {
                        work: work.clone(),
                        revision: record.revision,
                    },
                )?);
            }
            let stops = update
                .stop
                .iter()
                .map(|s| &s.target_id)
                .chain(update.add_work.iter().filter_map(|w| w.replaces.as_ref()))
                .collect::<std::collections::BTreeSet<_>>();
            for target in stops {
                let key = self.key(format!("stops/{}/{}", update.graph_id, digest(target)));
                let previous = self.read::<u64>(&key).await?.map(|(revision, _)| revision);
                mutations.push(self.mutation(key, previous, &record.revision)?);
            }
            self.commit(mutations).await?;
            Ok(CommittedUpdate {
                update,
                revision: record.revision,
                committed_at: now,
            })
        })
    }
    fn intent(
        &self,
        graph: &GraphId,
        work: &WorkId,
    ) -> BoxFuture<'_, Result<Option<Intent>, Error>> {
        let key = self.key(format!("intents/{graph}/{work}"));
        Box::pin(async move { Ok(self.read(&key).await?.map(|(_, intent)| intent)) })
    }
    fn commit_intent(&self, intent: Intent) -> BoxFuture<'_, Result<Intent, Error>> {
        Box::pin(async move {
            intent.validate()?;
            let key = self.key(format!("intents/{}/{}", intent.graph_id, intent.work_id));
            if let Some((_, previous)) = self.read::<Intent>(&key).await? {
                return if previous == intent {
                    Ok(previous)
                } else {
                    Err(Error::Conflict)
                };
            }
            let (revision, header) = self.header(&intent.graph_id).await?;
            if header.control.closed() {
                return Err(Error::Closed);
            }
            if header.control.schedule_revision != intent.schedule_revision
                || intent.request.session_id == header.control.epoch.root_session_id
            {
                return Err(Error::Conflict);
            }
            let (_, work): (_, WorkRecord) = self
                .required(&self.key(format!("work/{}/{}", intent.graph_id, intent.work_id)))
                .await?;
            for target in [intent.work_id.as_str(), intent.operator_id.as_str()] {
                if self
                    .read::<u64>(&self.key(format!("stops/{}/{}", intent.graph_id, digest(target))))
                    .await?
                    .is_some_and(|(_, stopped)| stopped >= work.revision)
                {
                    return Err(Error::Closed);
                }
            }
            self.commit(vec![
                self.write_header(revision, &header)?,
                self.mutation(key, None, &intent)?,
            ])
            .await?;
            Ok(intent)
        })
    }
    fn commit_wake(&self, wake: Wake) -> BoxFuture<'_, Result<Wake, Error>> {
        Box::pin(async move {
            wake.validate()?;
            let key = self.key(format!(
                "wakes/{}/{}",
                wake.graph_id,
                digest(&wake.snapshot_key)
            ));
            if let Some((_, previous)) = self.read::<Wake>(&key).await? {
                return if previous.snapshot_key == wake.snapshot_key
                    && previous.request.operation_id == wake.request.operation_id
                    && previous.request.session_id == wake.request.session_id
                {
                    Ok(previous)
                } else {
                    Err(Error::Conflict)
                };
            }
            let (revision, header) = self.header(&wake.graph_id).await?;
            if header.control.closed() {
                return Err(Error::Closed);
            }
            if wake.request.session_id != header.control.epoch.root_session_id {
                return Err(Error::Conflict);
            }
            self.commit(vec![
                self.write_header(revision, &header)?,
                self.mutation(key, None, &wake)?,
            ])
            .await?;
            Ok(wake)
        })
    }
}

fn decode<T: DeserializeOwned>(record: Record) -> Result<(u64, T), Error> {
    match record.data {
        Data::Present(value) => Ok((
            record.revision,
            serde_json::from_value(value).map_err(invalid)?,
        )),
        Data::Deleted => Err(invalid("Graph authority was deleted")),
    }
}
fn invalid(error: impl ToString) -> Error {
    Error::Persistence(error.to_string())
}
fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

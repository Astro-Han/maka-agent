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

use super::{Error, PreferenceSnapshot};
use crate::Preference;
use maka_plugins::storage::{Data, Mutation, Store, StoreError};
use std::{collections::BTreeMap, sync::Arc};

const KEY: &str = "preferences";

pub(super) struct Preferences(pub Arc<dyn Store>);

impl Preferences {
    pub async fn read(&self) -> Result<PreferenceSnapshot, Error> {
        let record = self.0.read(KEY.into()).await.map_err(stored)?;
        let revision = record.as_ref().map_or(0, |record| record.revision);
        let entries: BTreeMap<String, Preference> = match record.map(|record| record.data) {
            Some(Data::Present(value)) => serde_json::from_value(value)?,
            None | Some(Data::Deleted) => BTreeMap::new(),
        };
        for reference in entries.keys() {
            validate_reference(reference)?;
        }
        Ok(PreferenceSnapshot { revision, entries })
    }

    pub async fn compare_exchange(
        &self,
        expected_revision: u64,
        reference: String,
        preference: Preference,
    ) -> Result<(bool, PreferenceSnapshot), Error> {
        validate_reference(&reference)?;
        let mut current = self.read().await?;
        if current.revision != expected_revision {
            return Ok((false, current));
        }
        if preference == Preference::default() {
            current.entries.remove(&reference);
        } else {
            current.entries.insert(reference, preference);
        }
        let mutation = Mutation {
            key: KEY.into(),
            expected_revision: (expected_revision != 0).then_some(expected_revision),
            data: Data::Present(serde_json::to_value(&current.entries)?),
        };
        match self.0.batch(vec![mutation]).await {
            Ok(records) => {
                let [record]: [_; 1] = records.try_into().map_err(|_| {
                    Error::OutcomeUnknown("invalid preference commit receipt".into())
                })?;
                current.revision = record.revision;
                // The committed receipt is sufficient even if retirement now
                // prevents further reads. Never turn success into a second query.
                Ok((true, current))
            }
            Err(StoreError::Conflict { .. }) => Ok((false, self.read().await?)),
            Err(error) => Err(stored(error)),
        }
    }
}

fn validate_reference(reference: &str) -> Result<(), Error> {
    if reference.is_empty() || reference.len() > 512 || reference.chars().any(char::is_control) {
        Err(Error::Invalid("invalid Skill preference reference".into()))
    } else {
        Ok(())
    }
}

fn stored(error: StoreError) -> Error {
    match error {
        StoreError::Retired => Error::Retired,
        StoreError::OutcomeUnknown(message) => Error::OutcomeUnknown(message),
        error => Error::Source(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::future::BoxFuture;
    use maka_plugins::storage::Record;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct RetiringStore(AtomicBool);

    impl Store for RetiringStore {
        fn scan(
            &self,
            _: maka_plugins::storage::Scan,
        ) -> BoxFuture<'_, Result<maka_plugins::storage::Page, StoreError>> {
            unreachable!("preferences read exact keys")
        }
        fn read(&self, _: String) -> BoxFuture<'_, Result<Option<Record>, StoreError>> {
            Box::pin(async {
                if self.0.load(Ordering::SeqCst) {
                    return Err(StoreError::Retired);
                }
                // A deleted record still has a revision; recreating its value
                // must not be submitted as an absent-key write.
                Ok(Some(Record {
                    revision: 7,
                    data: Data::Deleted,
                }))
            })
        }

        fn batch(
            &self,
            mutations: Vec<Mutation>,
        ) -> BoxFuture<'_, Result<Vec<Record>, StoreError>> {
            Box::pin(async move {
                let [mutation]: [_; 1] = mutations.try_into().unwrap();
                assert_eq!(mutation.key, KEY);
                assert_eq!(mutation.expected_revision, Some(7));
                assert!(!self.0.swap(true, Ordering::SeqCst));
                Ok(vec![Record {
                    revision: 8,
                    data: mutation.data,
                }])
            })
        }
    }

    #[tokio::test]
    async fn a_committed_preference_survives_store_retirement() {
        let preferences = Preferences(Arc::new(RetiringStore(AtomicBool::new(false))));
        let preference = Preference {
            enabled: true,
            pinned: true,
        };
        let (committed, stale) = preferences
            .compare_exchange(0, "project:maka:review".into(), preference)
            .await
            .unwrap();
        assert!(!committed);
        assert_eq!(stale.revision, 7);
        assert!(stale.entries.is_empty());

        let (committed, current) = preferences
            .compare_exchange(7, "project:maka:review".into(), preference)
            .await
            .unwrap();
        assert!(committed);
        assert_eq!(current.revision, 8);
        assert_eq!(current.entries["project:maka:review"], preference);
        assert!(matches!(preferences.read().await, Err(Error::Retired)));
    }
}

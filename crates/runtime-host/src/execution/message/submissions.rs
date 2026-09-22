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

use super::super::{Executions, Result};
use maka_event_log::message_resolution::MessageResolution;
use maka_protocol::message::{ExecutionResolution, QueryInput};
use std::{collections::HashMap, sync::Mutex};

/// Input preparation releases the admission gate. Keep its identities visible
/// until the submit future settles, including concurrent retries of one message.
#[derive(Default)]
pub(in crate::execution) struct Submissions(Mutex<HashMap<(String, String), usize>>);

impl Submissions {
    pub(in crate::execution) fn track<'a>(
        &'a self,
        session: &str,
        message: &str,
    ) -> Submission<'a> {
        let key = (session.to_owned(), message.to_owned());
        *self.0.lock().unwrap().entry(key.clone()).or_default() += 1;
        Submission {
            submissions: self,
            key,
        }
    }

    fn contains(&self, session: &str, message: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .contains_key(&(session.to_owned(), message.to_owned()))
    }
}

pub(in crate::execution) struct Submission<'a> {
    submissions: &'a Submissions,
    key: (String, String),
}

impl Drop for Submission<'_> {
    fn drop(&mut self) {
        let mut entries = self.submissions.0.lock().unwrap();
        let count = entries.get_mut(&self.key).expect("live submission");
        *count -= 1;
        if *count == 0 {
            entries.remove(&self.key);
        }
    }
}

impl Executions {
    pub(crate) async fn query_message_executions(
        &self,
        input: QueryInput,
    ) -> Result<Vec<ExecutionResolution>> {
        // Plugin submissions transfer this same gate to their durable worker.
        // SQL absence alone cannot exclude a not-yet-committed admission.
        let _gate = self.lock_admission().await;
        let stored = self
            .log
            .message_resolutions(&input.session_id, &input.message_ids)
            .await
            .map_err(|error| self.message_storage_error(error))?;
        Ok(stored
            .into_iter()
            .filter_map(|resolution| match resolution {
                MessageResolution::Absent { message_id } => {
                    (!self.submissions.contains(&input.session_id, &message_id))
                        .then_some(ExecutionResolution::NotAdmitted { message_id })
                }
                MessageResolution::Pending { message_id } => {
                    Some(ExecutionResolution::Pending { message_id })
                }
                MessageResolution::Cancelled { message_id } => {
                    Some(ExecutionResolution::Cancelled { message_id })
                }
                MessageResolution::Owned {
                    message_id,
                    invocation,
                } => Some(ExecutionResolution::Owned {
                    message_id,
                    turn_id: invocation.turn_id,
                    run_id: invocation.run_id,
                }),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::Submissions;

    #[tokio::test]
    async fn cancelling_one_duplicate_keeps_the_other_preparation_visible() {
        let submissions = Submissions::default();
        let original = submissions.track("session", "message");
        let duplicate = async {
            let _duplicate = submissions.track("session", "message");
            std::future::pending::<()>().await;
        };
        let mut duplicate = Box::pin(duplicate);
        assert!(futures_util::poll!(&mut duplicate).is_pending());
        drop(original);
        assert!(submissions.contains("session", "message"));
        assert!(!submissions.contains("other-session", "message"));
        drop(duplicate);
        assert!(!submissions.contains("session", "message"));
    }
}

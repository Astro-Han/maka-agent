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

use crate::{
    app::{App, ConnectionState},
    pages::sessions::Detail,
};
use maka_protocol::{session::SessionStatus, turn::TurnState};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Activity {
    Idle,
    Working,
    Waiting,
    Unknown,
}
impl App {
    pub(crate) fn session_activity(&self, id: &str) -> Activity {
        if !matches!(self.connection, ConnectionState::Connected { .. }) {
            return Activity::Unknown;
        }
        if self.chat.session.as_deref() == Some(id) {
            if self.chat.error.is_some() {
                return Activity::Unknown;
            }
            if let Some(snapshot) = &self.chat.snapshot {
                if !snapshot.interactions.pending().is_empty() {
                    return Activity::Waiting;
                }
                if let Some(turn) = &snapshot.root_turn {
                    return match turn.state {
                        TurnState::Created(_) | TurnState::Admitted(_) | TurnState::Running(_) => {
                            Activity::Working
                        }
                        TurnState::WaitingForUser(_) => Activity::Waiting,
                        _ => Activity::Idle,
                    };
                }
                return status(snapshot.session.status);
            }
        }
        let detail = match &self.sessions.detail {
            Detail::Ready(item) if item.id == id => Some(item.as_ref()),
            _ => None,
        };
        self.sessions
            .items
            .iter()
            .chain(&self.inbox.items)
            .chain(detail)
            .filter(|item| item.id == id)
            .max_by_key(|item| item.status_updated_at.unwrap_or(item.activity_at))
            .map_or(Activity::Unknown, |item| {
                if item.status == SessionStatus::WaitingForUser {
                    Activity::Waiting
                } else if item
                    .live_run_state
                    .as_ref()
                    .is_some_and(|run| !run.running_turn_ids.is_empty())
                {
                    Activity::Working
                } else {
                    status(item.status)
                }
            })
    }
}
fn status(state: SessionStatus) -> Activity {
    match state {
        SessionStatus::Running => Activity::Working,
        SessionStatus::WaitingForUser => Activity::Waiting,
        _ => Activity::Idle,
    }
}

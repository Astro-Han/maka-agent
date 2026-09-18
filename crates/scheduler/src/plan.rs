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

mod edit;

use crate::{
    Error,
    authorization::Authorization,
    invalid,
    task::{Create, Creator, Effect, Intent, Outcome, Run, Status, Task},
};
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Misfire {
    #[default]
    Skip,
    Latest,
}

/// Persist before submitting to Host. A retry uses these frozen bytes and ID,
/// never the task's potentially edited title, effect or instruction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Fire {
    pub id: String,
    pub task_id: String,
    pub scheduled_at: i64,
    pub title: String,
    pub intent: Intent,
    pub effect: Effect,
    /// Notifications cannot be blindly replayed after an unknown external result.
    pub delivery_started: bool,
    #[serde(default)]
    pub authorization: Option<Authorization>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Plan {
    pub task: Task,
    pub timezone: String,
    pub misfire: Misfire,
    pub pending: Option<Fire>,
    #[serde(default)]
    pub authorization: Option<Authorization>,
}
impl Plan {
    pub(crate) fn waiting_notification(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|fire| matches!(fire.effect, Effect::Notify(_)) && !fire.delivery_started)
    }
    pub(crate) fn cancel_waiting_notification(&mut self) {
        if self.waiting_notification() {
            self.pending = None;
            self.task.last_error = None;
        }
    }
    pub fn validate(&self) -> Result<(), Error> {
        crate::task::text(&self.task.id, 160)?;
        if self
            .task
            .id
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        {
            return Err(invalid("invalid task identity"));
        }
        TimeZone::get(&self.timezone)?;
        if let Some(authorization) = &self.authorization {
            authorization.validate(&self.task.effect)?;
        }
        Create {
            title: self.task.title.clone(),
            intent_body: self.task.intent.body().into(),
            schedule: self.task.schedule.clone(),
            effect: self.task.effect.clone(),
            max_fires: self.task.max_fires,
            expires_at: self.task.expires_at,
        }
        .validate_fields()?;
        if self.task.created_at < 0
            || self.task.updated_at < self.task.created_at
            || self.task.runs.len() > 20
            || (self.task.status == Status::Active && self.task.next_fire_at.is_none())
        {
            return Err(invalid("invalid persisted task state"));
        }
        if let Some(fire) = &self.pending {
            crate::task::text(&fire.id, 256)?;
            fire.effect.validate()?;
            if let Some(authorization) = &fire.authorization {
                authorization.validate(&fire.effect)?;
            }
            if fire.task_id != self.task.id
                || fire.scheduled_at < self.task.created_at
                || fire.intent.body().chars().count() > 8000
            {
                return Err(invalid("invalid persisted trigger"));
            }
        }
        Ok(())
    }

    pub fn create(
        id: String,
        input: Create,
        creator: Creator,
        timezone: String,
        now: i64,
    ) -> Result<Self, Error> {
        crate::task::text(&id, 160)?;
        if let Creator::Agent { session_id } = &creator {
            crate::task::text(session_id, 160)?;
        }
        input.validate(now)?;
        let zone = TimeZone::get(&timezone)?;
        let next = input
            .schedule
            .next_after(now, &zone)?
            .ok_or_else(|| invalid("schedule has no trigger within one year"))?;
        if input.expires_at.is_some_and(|expires| next >= expires) {
            return Err(invalid("schedule first fires after expiry"));
        }
        Ok(Self {
            task: Task {
                id,
                title: input.title.trim().into(),
                intent: Intent::Text {
                    body: input.intent_body.trim().into(),
                },
                schedule: input.schedule,
                effect: input.effect,
                status: Status::Active,
                next_fire_at: Some(next),
                last_fire_at: None,
                fire_count: 0,
                max_fires: input.max_fires,
                expires_at: input.expires_at,
                created_by: creator,
                created_at: now,
                updated_at: now,
                runs: vec![],
                last_error: None,
            },
            timezone,
            misfire: Misfire::Skip,
            pending: None,
            authorization: None,
        })
    }
    /// Reopening or enabling skips missed triggers unless catch-up was selected.
    /// A previously committed pending fire must still be reconciled: it may
    /// already have been accepted by Host.
    pub fn recover(&mut self, now: i64) -> Result<(), Error> {
        if self.pending.is_some() || self.task.status != Status::Active {
            return Ok(());
        }
        let before = (self.task.status, self.task.next_fire_at);
        if self.task.expires_at.is_some_and(|expires| now >= expires) {
            self.task.status = Status::Expired;
            self.task.next_fire_at = None;
        } else if self.misfire == Misfire::Skip && self.task.next_fire_at.is_some_and(|at| at < now)
        {
            self.advance(now)?;
        }
        if before != (self.task.status, self.task.next_fire_at) {
            self.task.updated_at = now;
        }
        Ok(())
    }
    /// Returns the same intent while settlement is pending. The repository must
    /// commit the changed Plan before the caller may dispatch the Fire.
    pub fn claim(&mut self, now: i64) -> Result<Option<&Fire>, Error> {
        if self.pending.is_some() {
            return Ok(self.pending.as_ref());
        }
        if self.task.status != Status::Active {
            return Ok(None);
        }
        if self.task.expires_at.is_some_and(|expires| now >= expires) {
            self.task.status = Status::Expired;
            self.task.next_fire_at = None;
            self.task.updated_at = now;
            return Ok(None);
        }
        let Some(at) = self.task.next_fire_at.filter(|at| *at <= now) else {
            return Ok(None);
        };
        self.pending = Some(Fire {
            id: format!("fire-{}", uuid::Uuid::new_v4()),
            task_id: self.task.id.clone(),
            scheduled_at: at,
            title: self.task.title.clone(),
            intent: self.task.intent.clone(),
            effect: self.task.effect.clone(),
            delivery_started: false,
            authorization: self.authorization.clone(),
        });
        self.task.updated_at = now;
        Ok(self.pending.as_ref())
    }
    pub fn settle(&mut self, run: Run) -> Result<(), Error> {
        let mut next = self.clone();
        next.settle_inner(run)?;
        *self = next;
        Ok(())
    }

    fn settle_inner(&mut self, run: Run) -> Result<(), Error> {
        let fire = self
            .pending
            .as_ref()
            .ok_or_else(|| invalid("no pending trigger"))?;
        if run.id != fire.id || run.at < fire.scheduled_at || run.message.chars().count() > 1024 {
            return Err(invalid("run does not settle the pending trigger"));
        }
        let paused = self.task.status == Status::Paused;
        self.task.last_fire_at = Some(run.at);
        self.task.updated_at = run.at;
        self.task.fire_count = self
            .task
            .fire_count
            .checked_add(1)
            .ok_or_else(|| invalid("fire count exhausted"))?;
        self.task.last_error = (run.outcome != Outcome::Ok).then(|| run.message.clone());
        self.task.runs.insert(0, run);
        self.task.runs.truncate(20);
        self.task.bound_history()?;
        self.pending = None;
        self.advance(self.task.updated_at)?;
        if paused && self.task.status == Status::Active {
            self.task.status = Status::Paused;
        }
        Ok(())
    }
    fn advance(&mut self, after: i64) -> Result<(), Error> {
        let next = self
            .task
            .schedule
            .next_after(after, &TimeZone::get(&self.timezone)?)?;
        let expired = self
            .task
            .expires_at
            .is_some_and(|expires| after >= expires || next.is_some_and(|at| at >= expires));
        let completed = next.is_none()
            || (matches!(self.task.schedule, crate::schedule::Schedule::Once { .. })
                && self.task.fire_count > 0)
            || self
                .task
                .max_fires
                .is_some_and(|max| self.task.fire_count >= max);
        self.task.status = if expired {
            Status::Expired
        } else if completed {
            Status::Completed
        } else {
            Status::Active
        };
        self.task.next_fire_at = (self.task.status == Status::Active)
            .then_some(next)
            .flatten();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::Schedule;

    #[test]
    fn recovery_skips_misfires_but_replays_frozen_pending_intent_until_settlement() {
        let input = Create {
            title: "Original".into(),
            intent_body: "Work".into(),
            schedule: Schedule::Interval {
                every_seconds: 10,
                start_at: 1000,
            },
            effect: Effect::SessionResume {
                session_id: "session".into(),
            },
            max_fires: Some(1),
            expires_at: None,
        };
        let mut plan =
            Plan::create("task".into(), input, Creator::User, "UTC".into(), 1000).unwrap();
        plan.recover(31_000).unwrap();
        assert_eq!(plan.task.next_fire_at, Some(41_000));
        assert_eq!(plan.task.fire_count, 0);
        let fire = plan.claim(41_000).unwrap().unwrap().clone();
        let mut reopened: Plan =
            serde_json::from_slice(&serde_json::to_vec(&plan).unwrap()).unwrap();
        reopened.task.title = "Edited".into();
        reopened.task.intent = Intent::Text {
            body: "Changed".into(),
        };
        reopened.recover(61_000).unwrap();
        assert_eq!(reopened.claim(61_000).unwrap(), Some(&fire));
        let run = Run {
            id: fire.id.clone(),
            at: 61_000,
            outcome: Outcome::Ok,
            message: "admitted".into(),
            session_id: Some("session".into()),
            run_id: Some("run".into()),
        };
        reopened.settle(run.clone()).unwrap();
        assert_eq!(reopened.task.status, Status::Completed);
        assert_eq!(reopened.task.fire_count, 1);
        assert!(reopened.claim(71_000).unwrap().is_none());
        assert!(reopened.settle(run).is_err());
    }
}

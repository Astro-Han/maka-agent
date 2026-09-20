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

use super::Owner;
use crate::{
    Error,
    delivery::Delivery,
    plan::Plan,
    task::{Effect, Outcome, Run},
};
use futures_util::future::BoxFuture;
use std::time::Duration;
use tokio::time::Instant;

pub(super) type Job = BoxFuture<'static, Completed>;
pub(super) struct Completed {
    pub task_id: String,
    fire_id: String,
    delivery: Delivery,
}
pub(super) struct Retry {
    pub at: Instant,
    pub fire_id: String,
    delay: Duration,
}
impl Owner {
    pub(super) async fn launch(&mut self, now: i64) -> Result<(), Error> {
        let ids = self
            .controller
            .catalog
            .plans
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for id in ids {
            if self.jobs.len() >= 8 {
                break;
            }
            if self.active.contains_key(&id)
                || self
                    .retries
                    .get(&id)
                    .is_some_and(|retry| retry.at > Instant::now())
            {
                continue;
            }
            let original = &self.controller.catalog.plans[&id].plan;
            let mut plan = original.clone();
            let fire = plan.claim(now)?.cloned();
            let Some(mut fire) = fire else {
                if &plan != original {
                    self.save(plan).await?;
                }
                continue;
            };
            if matches!(fire.effect, Effect::Notify(_)) {
                if fire.delivery_started {
                    // We cannot distinguish "sent, lost acknowledgement" from
                    // "crashed before sending"; never silently send it twice.
                    self.finish(Completed {
                        task_id: id,
                        fire_id: fire.id,
                        delivery: Delivery::Blocked(
                            "Notification outcome unknown; not replayed".into(),
                        ),
                    })
                    .await?;
                    continue;
                }
                fire.delivery_started = true;
                plan.pending = Some(fire.clone());
            }
            if &plan != original {
                self.save(plan).await?;
            }
            let dispatcher = self.dispatcher.clone();
            let stop = tokio_util::sync::CancellationToken::new();
            self.active.insert(id.clone(), stop.clone());
            self.jobs.push(Box::pin(async move {
                let _cancel_on_drop = stop.clone().drop_guard();
                let fire_id = fire.id.clone();
                let notification = matches!(fire.effect, Effect::Notify(_));
                let result =
                    tokio::time::timeout(Duration::from_secs(30), dispatcher.dispatch(fire, stop))
                        .await;
                let delivery = match result {
                    Ok(Delivery::Retry(_)) | Err(_) if notification => {
                        Delivery::Blocked("Notification outcome unknown; not replayed".into())
                    }
                    Ok(result) => result,
                    Err(_) => Delivery::Retry(
                        "Host admission timed out; retrying the same operation".into(),
                    ),
                };
                Completed {
                    task_id: id,
                    fire_id,
                    delivery,
                }
            }));
        }
        Ok(())
    }
    pub(super) async fn finish(&mut self, completed: Completed) -> Result<(), Error> {
        let Some(saved) = self.controller.catalog.plans.get(&completed.task_id) else {
            return Ok(());
        };
        let mut plan = saved.plan.clone();
        let Some(fire) = &plan.pending else {
            return Ok(());
        };
        if fire.id != completed.fire_id {
            return Ok(());
        }
        let at = self
            .clock
            .now()
            .max(fire.scheduled_at)
            .max(plan.task.updated_at);
        let deferred = matches!(&completed.delivery, Delivery::Deferred(_));
        let (outcome, message, session_id, run_id) = match completed.delivery {
            Delivery::Accepted { session_id, run_id } => (
                Outcome::Ok,
                "Execution admitted".into(),
                Some(session_id),
                Some(run_id),
            ),
            Delivery::Notified => (Outcome::Ok, "Notification sent".into(), None, None),
            Delivery::Failed(message) => (Outcome::Failed, message, None, None),
            Delivery::Blocked(message) => (Outcome::Blocked, message, None, None),
            Delivery::Retry(message) | Delivery::Deferred(message) => {
                if deferred {
                    plan.pending.as_mut().unwrap().delivery_started = false;
                    if plan.task.status != crate::task::Status::Active
                        && plan.waiting_notification()
                    {
                        plan.cancel_waiting_notification();
                        self.retries.remove(&completed.task_id);
                        return self.save(plan).await;
                    }
                }
                let delay = self
                    .retries
                    .get(&completed.task_id)
                    .map_or(Duration::from_millis(250), |retry| {
                        (retry.delay * 2).min(Duration::from_secs(30))
                    });
                self.retries.insert(
                    completed.task_id,
                    Retry {
                        at: Instant::now() + delay,
                        fire_id: completed.fire_id,
                        delay,
                    },
                );
                plan.task.last_error = Some(message.chars().take(1024).collect());
                plan.task.updated_at = at;
                return self.save(plan).await;
            }
        };
        plan.settle(Run {
            id: completed.fire_id,
            at,
            outcome,
            message: message.chars().take(1024).collect(),
            session_id,
            run_id,
        })?;
        self.save(plan).await?;
        self.retries.remove(&completed.task_id);
        Ok(())
    }
    async fn save(&mut self, plan: Plan) -> Result<(), Error> {
        self.controller
            .repository
            .save(&mut self.controller.catalog, plan)
            .await
    }
}

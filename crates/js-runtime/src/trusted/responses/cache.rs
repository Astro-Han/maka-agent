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

use super::output::Output;
use super::{Result, error};
use crate::trusted::budget;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub(super) const CACHE_LIMIT: u32 = 32 * 1024 * 1024;

/// One reconstructable wire context, never a second semantic history.
pub(super) struct Baseline {
    pub body: Value,
    pub response_id: Option<String>,
    permit: OwnedSemaphorePermit,
    output: Output,
}

pub(super) struct Prepared {
    pub full: Value,
    pub delta: Option<Value>,
    // Keep the old reservation through reconnect/send; reconstruction moves
    // the old input rather than copying its images into an unbudgeted cache.
    prior: Option<OwnedSemaphorePermit>,
}

impl Prepared {
    pub fn new(mut body: Value, baseline: Option<Baseline>) -> Result<Self> {
        let object = body
            .as_object_mut()
            .ok_or_else(|| error("invalid Responses request body"))?;
        let previous = object.remove("previous_response_id");
        let mut prepared = Self {
            full: body,
            delta: None,
            prior: None,
        };
        if let Some(previous) = previous.filter(|id| !id.is_null()) {
            let Some(mut baseline) =
                baseline.filter(|baseline| previous.as_str() == baseline.response_id.as_deref())
            else {
                return Err(error("Responses continuation has no confirmed baseline"));
            };
            let properties_match = properties(&prepared.full).count()
                == properties(&baseline.body).count()
                && properties(&prepared.full)
                    .all(|(key, value)| baseline.body.get(key) == Some(value));
            if properties_match {
                let mut delta = prepared.full.clone();
                delta["previous_response_id"] = previous;
                prepared.delta = Some(delta);
            }
            let input = baseline.body["input"]
                .as_array_mut()
                .ok_or_else(|| error("Responses baseline has no input"))?;
            let tail = prepared.full["input"]
                .as_array_mut()
                .ok_or_else(|| error("Responses continuation has no input"))?;
            input.append(tail);
            prepared.full["input"] = baseline.body["input"].take();
            prepared.prior = Some(baseline.permit);
        }
        budget::bytes(&prepared.full, CACHE_LIMIT)
            .map_err(|_| error("Responses WebSocket input exceeds 32 MiB"))?;
        Ok(prepared)
    }

    pub fn cache(self, budget: &Arc<Semaphore>) -> Option<Baseline> {
        let bytes = budget::bytes(&self.full, CACHE_LIMIT).ok()?;
        let permit = match self.prior {
            Some(mut prior) => {
                let reserved = prior.num_permits() as u32;
                if bytes > reserved {
                    prior.merge(
                        budget
                            .clone()
                            .try_acquire_many_owned(bytes - reserved)
                            .ok()?,
                    );
                } else if bytes < reserved {
                    drop(prior.split((reserved - bytes) as usize));
                }
                prior
            }
            None => budget.clone().try_acquire_many_owned(bytes).ok()?,
        };
        Some(Baseline {
            body: self.full,
            response_id: None,
            permit,
            output: Output::default(),
        })
    }
}

impl Baseline {
    pub fn observe(&mut self, event: &mut Value, budget: &Arc<Semaphore>) -> Option<()> {
        self.output.observe(event, &mut self.permit, budget)
    }

    pub fn complete(mut self, response: &mut Value, budget: &Arc<Semaphore>) -> Option<Self> {
        let id = response["id"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 1024)?
            .to_owned();
        let output = response["output"].as_array_mut()?;
        let input = self.body["input"].as_array_mut()?;
        if output.is_empty() {
            input.extend(self.output.finish()?);
        } else {
            input.append(output);
            drop(self.output);
        }
        let mut baseline = Prepared {
            full: self.body,
            delta: None,
            prior: Some(self.permit),
        }
        .cache(budget)?;
        baseline.response_id = Some(id);
        Some(baseline)
    }
}

fn properties(body: &Value) -> impl Iterator<Item = (&String, &Value)> {
    body.as_object()
        .into_iter()
        .flat_map(|object| object.iter())
        .filter(|(key, _)| !matches!(key.as_str(), "input" | "previous_response_id"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cache_reservations_bound_growth_and_release_on_failure_or_drop() {
        let budget = Arc::new(Semaphore::new(512));
        let body =
            json!({"model":"test","input":[{"role":"user","content":"first"}],"stream":true});
        let mut response = json!({"id":"resp_1","output":[{"type":"function_call","call_id":"call_1","arguments":"{}"}]});
        let baseline = Prepared::new(body.clone(), None)
            .unwrap()
            .cache(&budget)
            .unwrap()
            .complete(&mut response, &budget)
            .unwrap();
        let held = 512 - budget.available_permits();
        assert!(held > 0 && held < 512);
        let prepared = Prepared::new(
            json!({"model":"test","stream":true,"previous_response_id":"resp_1",
            "input":[{"type":"function_call_output","call_id":"call_1","output":"x".repeat(512)}]}),
            Some(baseline),
        )
        .unwrap();
        assert!(prepared.delta.is_some());
        assert_eq!(prepared.full["input"].as_array().unwrap().len(), 3);
        assert_eq!(
            budget.available_permits(),
            512 - held,
            "old input stays reserved through reconnect/send"
        );
        assert!(
            prepared.cache(&budget).is_none(),
            "budget exhaustion disables optimization without waiting"
        );
        assert_eq!(budget.available_permits(), 512);
        let baseline = Prepared::new(body, None).unwrap().cache(&budget).unwrap();
        assert!(
            baseline
                .complete(
                    &mut json!({"id":"resp_2","output":["x".repeat(512)]}),
                    &budget
                )
                .is_none()
        );
        assert_eq!(budget.available_permits(), 512);
    }
}

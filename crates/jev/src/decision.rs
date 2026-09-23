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

use crate::settings::{Repository, Secrets, Settings};
use futures_util::future::BoxFuture;
use maka_plugins::{call, http, preferences::Preferences, services::method};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

/// Consumers inject this Service and forward their admitted scope.
pub const SERVICE: &str = "maka.jev.evaluate";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evaluation {
    pub state: Value,
    pub questions: BTreeMap<String, Question>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Question {
    Noul {
        instructions: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<BTreeMap<String, Value>>,
    },
    Choice {
        instructions: Value,
        criteria: BTreeMap<String, Value>,
    },
    Score {
        instructions: Value,
        criteria: Vec<Value>,
    },
}
fn structured(v: &Value) -> bool {
    v.is_string() || v.is_object() || v.is_array()
}
impl Evaluation {
    pub fn validate(&self) -> Result<(), Error> {
        if !structured(&self.state)
            || self.questions.is_empty()
            || self.questions.len() > 64
            || serde_json::to_vec(self).map_err(|_| Error::Input)?.len() > 32768
        {
            return Err(Error::Input);
        }
        for (id, question) in &self.questions {
            if id.is_empty() || id.len() > 128 {
                return Err(Error::Input);
            }
            let valid = match question {
                Question::Noul {
                    instructions,
                    criteria,
                } => {
                    structured(instructions)
                        && criteria.as_ref().is_none_or(|c| {
                            c.iter().all(|(k, v)| {
                                matches!(k.as_str(), "true" | "false") && structured(v)
                            })
                        })
                }
                Question::Choice {
                    instructions,
                    criteria,
                } => {
                    structured(instructions)
                        && (1..=255).contains(&criteria.len())
                        && criteria.iter().all(|(k, v)| {
                            !k.is_empty() && k.len() <= 128 && (v.is_null() || structured(v))
                        })
                }
                Question::Score {
                    instructions,
                    criteria,
                } => {
                    structured(instructions)
                        && (2..=10).contains(&criteria.len())
                        && criteria.iter().all(structured)
                }
            };
            if !valid {
                return Err(Error::Input);
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationResult {
    pub model: String,
    pub answers: BTreeMap<String, Answer>,
    pub usage: Usage,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Answer {
    Noul {
        noul: f64,
    },
    Choice {
        choice: String,
        confidence: f64,
        probabilities: BTreeMap<String, f64>,
    },
    Score {
        score: f64,
        confidence: f64,
        probabilities: BTreeMap<String, f64>,
        legend: BTreeMap<String, String>,
    },
}
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Jev request exceeds the supported evaluation contract")]
    Input,
    #[error("Jev is disabled or its endpoint credential is not configured")]
    Unavailable,
    #[error("Jev network access is not authorized")]
    Denied,
    #[error("Jev request cancelled")]
    Cancelled,
    #[error("Jev request timed out; the provider may have processed it")]
    Timeout,
    #[error("Jev network request failed; the provider may have processed it")]
    Network,
    #[error("Jev returned HTTP {0}")]
    Status(u16),
    #[error("Jev returned an invalid or oversized response")]
    Response,
    #[error("Jev resource settlement is unconfirmed")]
    Settlement,
}
impl Error {
    pub(crate) fn outcome_unknown(&self) -> bool {
        matches!(
            self,
            Self::Network | Self::Timeout | Self::Cancelled | Self::Settlement
        )
    }
}
#[derive(Clone)]
pub(crate) struct Jev {
    pub settings: Repository,
    pub http: Arc<dyn http::Client>,
    pub preferences: Arc<dyn Preferences>,
}
impl Jev {
    pub async fn evaluate(
        &self,
        parent: &call::Scope,
        input: Evaluation,
    ) -> Result<EvaluationResult, Error> {
        input.validate()?;
        let (_, settings) = self
            .settings
            .settings()
            .await
            .map_err(|_| Error::Unavailable)?;
        self.configured(parent, input, settings).await
    }
    pub async fn configured(
        &self,
        parent: &call::Scope,
        input: Evaluation,
        settings: Settings,
    ) -> Result<EvaluationResult, Error> {
        input.validate()?;
        settings.validate().map_err(|_| Error::Input)?;
        if !settings.enabled
            || self
                .preferences
                .read()
                .await
                .map_err(|_| Error::Unavailable)?
                .privacy
                .incognito_active
        {
            return Err(Error::Unavailable);
        }
        let key = self
            .settings
            .credentials
            .read(settings.credential_key())
            .await
            .map_err(|_| Error::Unavailable)?
            .and_then(|r| r.secret)
            .filter(|s| !s.is_empty())
            .ok_or(Error::Unavailable)?;
        let secrets: Secrets = serde_json::from_str(&key).map_err(|_| Error::Unavailable)?;
        secrets.validate().map_err(|_| Error::Unavailable)?;
        let owned = call::Owned::new(parent.child().map_err(|_| Error::Denied)?);
        let scope = owned.scope();
        let result = tokio::select! {
            biased;
            _ = scope.cancellation.cancelled() => Err(Error::Cancelled),
            result = tokio::time::timeout(Duration::from_millis(settings.timeout_ms), self.request(&scope, &settings, &secrets, &input)) => result.unwrap_or(Err(Error::Timeout)),
        };
        scope.cancellation.cancel();
        owned.finish().await.map_err(|_| Error::Settlement)?;
        result
    }
    async fn request(
        &self,
        scope: &call::Scope,
        settings: &Settings,
        secrets: &Secrets,
        input: &Evaluation,
    ) -> Result<EvaluationResult, Error> {
        let response = self
            .http
            .request(
                scope.clone(),
                http::Request {
                    url: settings.url.clone(),
                    method: http::Method::Post,
                    headers: secrets.request_headers().map_err(|_| Error::Input)?,
                    body: serde_json::to_vec(&json!({"model": settings.model, "state":input.state,
                "questions":input.questions}))
                    .map_err(|_| Error::Input)?,
                },
            )
            .await
            .map_err(network)?;
        let result = async {
            if !(200..300).contains(&response.head.status) {
                return Err(Error::Status(response.head.status));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.body.next().await.map_err(network)? {
                if chunk.len() > 65536 - bytes.len() {
                    return Err(Error::Response);
                }
                bytes.extend_from_slice(&chunk);
            }
            decode(&bytes, input)
        }
        .await;
        // A redirect is a response, never permission to forward this endpoint's key.
        response.body.close().await.map_err(network)?;
        result
    }
}
impl method::Method<Evaluation, EvaluationResult> for Jev {
    fn call(
        &self,
        input: Evaluation,
        context: method::Context,
    ) -> BoxFuture<'_, Result<EvaluationResult, method::Error>> {
        Box::pin(async move {
            let scope = context.invocation.ok_or_else(|| {
                method::Error::Invalid("Jev requires an admitted caller scope".into())
            })?;
            let evaluation = self.evaluate(&scope, input);
            tokio::pin!(evaluation);
            let result = tokio::select! {
                biased;
                _ = context.cancellation.cancelled() => {
                    scope.cancellation.cancel();
                    evaluation.await
                },
                result = &mut evaluation => result,
            };
            result.map_err(|e| {
                if e.outcome_unknown() {
                    method::Error::OutcomeUnknown(e.to_string())
                } else {
                    method::Error::Failed(e.to_string())
                }
            })
        })
    }
}
fn network(e: http::Error) -> Error {
    match e {
        http::Error::Denied => Error::Denied,
        http::Error::CleanupUnconfirmed => Error::Settlement,
        _ => Error::Network,
    }
}
fn decode(bytes: &[u8], input: &Evaluation) -> Result<EvaluationResult, Error> {
    let result: EvaluationResult = serde_json::from_slice(bytes).map_err(|_| Error::Response)?;
    if result.model.is_empty()
        || result.model.len() > 256
        || result.answers.len() != input.questions.len()
        || result.usage.input_tokens >= (1 << 53)
        || result.usage.output_tokens >= (1 << 53)
    {
        return Err(Error::Response);
    }
    for (id, question) in &input.questions {
        let valid = match (question, result.answers.get(id)) {
            (Question::Noul { .. }, Some(Answer::Noul { noul })) => probability(*noul),
            (
                Question::Choice { criteria, .. },
                Some(Answer::Choice {
                    choice,
                    confidence,
                    probabilities,
                }),
            ) => {
                criteria.contains_key(choice)
                    && probability(*confidence)
                    && distribution(probabilities, criteria.keys().map(String::as_str))
            }
            (
                Question::Score { criteria, .. },
                Some(Answer::Score {
                    score,
                    confidence,
                    probabilities,
                    legend,
                }),
            ) => {
                let keys: Vec<_> = (0..criteria.len()).map(|i| i.to_string()).collect();
                let weighted: f64 = keys
                    .iter()
                    .enumerate()
                    .map(|(i, k)| i as f64 * probabilities.get(k).copied().unwrap_or(0.0))
                    .sum();
                score.is_finite()
                    && *score >= 0.0
                    && *score <= (criteria.len() - 1) as f64
                    && probability(*confidence)
                    && legend.len() == keys.len()
                    && keys.iter().all(|k| legend.contains_key(k))
                    && distribution(probabilities, keys.iter().map(String::as_str))
                    && (weighted - score).abs() <= 0.02
            }
            _ => false,
        };
        if !valid {
            return Err(Error::Response);
        }
    }
    Ok(result)
}
fn distribution<'a>(p: &BTreeMap<String, f64>, keys: impl Iterator<Item = &'a str>) -> bool {
    let keys: Vec<_> = keys.collect();
    p.len() == keys.len()
        && keys
            .iter()
            .all(|k| p.get(*k).is_some_and(|v| probability(*v)))
        && (p.values().sum::<f64>() - 1.0).abs() <= 0.01
}
fn probability(n: f64) -> bool {
    n.is_finite() && (0.0..=1.0).contains(&n)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_question_types_preserve_uncertainty_and_usage() {
        let input: Evaluation = serde_json::from_value(json!({"state":{"test":true},"questions":{
            "binary":{"type":"noul","instructions":["yes?"]},
            "choice":{"type":"choice","instructions":"pick","criteria":{"yes":null,"no":{"description":"no"}}},
            "score":{"type":"score","instructions":"rate","criteria":["low","high"]}
        }})).unwrap();
        input.validate().unwrap();
        let mut wire = json!({"model":"jev-latest","usage":{"input_tokens":10,"output_tokens":5},"answers":{
            "binary":{"type":"noul","noul":0.7},
            "choice":{"type":"choice","choice":"yes","confidence":0.51,"probabilities":{"yes":0.51,"no":0.49}},
            "score":{"type":"score","score":0.8,"confidence":0.6,"probabilities":{"0":0.2,"1":0.8},"legend":{"0":"low","1":"high"}}
        }});
        assert_eq!(
            decode(&serde_json::to_vec(&wire).unwrap(), &input)
                .unwrap()
                .usage
                .input_tokens,
            10
        );
        wire["answers"]["choice"]["choice"] = json!("foreign");
        assert!(decode(&serde_json::to_vec(&wire).unwrap(), &input).is_err());
        wire["answers"]["choice"]["choice"] = json!("yes");
        wire["answers"]["score"]["score"] = json!(0.1);
        assert!(decode(&serde_json::to_vec(&wire).unwrap(), &input).is_err());
    }
}

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
    ProtocolError, Result,
    codec::{exact, record, shaped, string},
};
use maka_runtime::configuration::{onboarding::*, validation};
use serde_json::Value;

pub fn decode_input(value: &Value, save: bool) -> Result<(OnboardingInput, Vec<String>)> {
    let row = record(value, "onboarding input")?;
    exact(
        row,
        if save {
            &["target", "apiKey", "baseUrl", "enabledModelIds"]
        } else {
            &["target", "apiKey", "baseUrl"]
        },
    )?;
    let target = record(&row["target"], "onboarding target")?;
    let target = match target.get("kind").and_then(Value::as_str) {
        Some("create") => {
            shaped(target, &["kind", "providerType"], &["slug", "name"])?;
            let provider_type = string(&target["providerType"], "provider type", 128)?;
            validation::provider_auth_kind(&provider_type).map_err(ProtocolError::invalid)?;
            let slug = target
                .get("slug")
                .map(|v| {
                    let slug = string(v, "slug", 64)?;
                    validation::slug(&slug).map_err(ProtocolError::invalid)?;
                    Ok(slug)
                })
                .transpose()?;
            let name = target
                .get("name")
                .map(|v| text(v, 128, false))
                .transpose()?;
            OnboardingTarget::Create {
                provider_type,
                slug,
                name,
            }
        }
        Some("existing") => {
            exact(target, &["kind", "connectionId"])?;
            let connection_id = string(&target["connectionId"], "connection id", 128)?;
            if !connection_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            {
                return Err(ProtocolError::invalid("Invalid connection id"));
            }
            OnboardingTarget::Existing { connection_id }
        }
        _ => return Err(ProtocolError::invalid("Invalid onboarding target")),
    };
    let api_key = nullable(&row["apiKey"], 64 * 1024)?;
    let base_url = nullable(&row["baseUrl"], 2048)?;
    let mut enabled = Vec::new();
    if save {
        let ids = row["enabledModelIds"]
            .as_array()
            .filter(|v| v.len() <= 2048)
            .ok_or_else(|| ProtocolError::invalid("Invalid onboarding model selection"))?;
        for value in ids {
            let id = text(value, 512, true)?;
            if enabled.contains(&id) {
                return Err(ProtocolError::invalid("Duplicate onboarding model"));
            }
            enabled.push(id);
        }
    }
    Ok((
        OnboardingInput {
            target,
            api_key,
            base_url,
        },
        enabled,
    ))
}

pub fn decode_verify_result(value: &Value) -> Result<OnboardingVerifyResult> {
    let result: OnboardingVerifyResult = serde_json::from_value(value.clone())
        .map_err(|_| ProtocolError::invalid("Invalid onboarding verification result"))?;
    match &result {
        OnboardingVerifyResult::Verified { models } => {
            if models.is_empty() {
                return Err(ProtocolError::invalid("Empty verified models"));
            }
            for model in models {
                model.validate().map_err(ProtocolError::invalid)?;
            }
        }
        OnboardingVerifyResult::Rejected {
            reason: OnboardingRejection::ModelUnavailable | OnboardingRejection::Superseded,
        } => {
            return Err(ProtocolError::invalid(
                "Invalid onboarding verification rejection",
            ));
        }
        _ => {}
    }
    Ok(result)
}

pub fn decode_save_result(value: &Value) -> Result<OnboardingSaveResult> {
    // The shared basis decoder accepts JSON 1.0 as an integer revision.
    let mut value = value.clone();
    if value["kind"] == "saved" {
        let revision =
            crate::codec::count(&value["connection"]["revision"], "connection revision")?;
        value["connection"]["revision"] = revision.into();
    }
    let result: OnboardingSaveResult = serde_json::from_value(value)
        .map_err(|_| ProtocolError::invalid("Invalid onboarding save result"))?;
    if let OnboardingSaveResult::Saved { connection } = &result {
        validation::basis(&connection.basis()).map_err(ProtocolError::invalid)?;
        validation::slug(&connection.slug).map_err(ProtocolError::invalid)?;
        validation::provider_auth_kind(&connection.provider_type)
            .map_err(ProtocolError::invalid)?;
    }
    Ok(result)
}

fn nullable(value: &Value, max: usize) -> Result<Option<String>> {
    if value.is_null() {
        Ok(None)
    } else {
        string(value, "onboarding field", max).map(Some)
    }
}
fn text(value: &Value, max: usize, nonempty: bool) -> Result<String> {
    let text = value
        .as_str()
        .ok_or_else(|| ProtocolError::invalid("Invalid onboarding text"))?;
    validation::text(text, max, nonempty).map_err(ProtocolError::invalid)?;
    Ok(text.to_owned())
}

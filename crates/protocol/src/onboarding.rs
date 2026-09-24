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
    codec::{exact, record},
};
use maka_runtime::configuration::{onboarding::*, validation};
use serde_json::Value;

pub fn decode_input(value: &Value, save: bool) -> Result<(OnboardingInput, Vec<String>)> {
    let row = record(value, "onboarding input")?;
    exact(
        row,
        if save {
            &["target", "enabledModelIds"]
        } else {
            &["target"]
        },
    )?;
    let mut input = value.clone();
    let enabled: Vec<String> = if save {
        let ids = input
            .as_object_mut()
            .unwrap()
            .remove("enabledModelIds")
            .unwrap();
        let ids: Vec<String> = serde_json::from_value(ids)
            .map_err(|_| ProtocolError::invalid("Invalid onboarding model selection"))?;
        validation::model_ids(&ids).map_err(ProtocolError::invalid)?;
        ids
    } else {
        Vec::new()
    };
    let input: OnboardingInput = serde_json::from_value(input)
        .map_err(|_| ProtocolError::invalid("Invalid onboarding input"))?;
    input
        .target
        .validate_create_identity()
        .map_err(ProtocolError::invalid)?;
    Ok((input, enabled))
}

pub fn decode_verify_result(value: &Value) -> Result<OnboardingVerifyResult> {
    let result: OnboardingVerifyResult = serde_json::from_value(value.clone())
        .map_err(|_| ProtocolError::invalid("Invalid onboarding verification result"))?;
    if let OnboardingVerifyResult::Verified { models } = &result {
        if models.is_empty() {
            return Err(ProtocolError::invalid("Empty verified models"));
        }
        for model in models {
            model.validate().map_err(ProtocolError::invalid)?;
        }
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
        connection
            .provider
            .validate()
            .map_err(ProtocolError::invalid)?;
    }
    Ok(result)
}

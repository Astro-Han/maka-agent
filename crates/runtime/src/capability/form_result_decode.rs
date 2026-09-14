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

use super::{
    form::*,
    form_decode::{ensure, invalid, number, record, shaped, strings, text},
    form_validation as validation,
};
use serde_json::Value;
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, &'static str>;
pub fn decode_form_result(value: &Value) -> Result<FormResult> {
    decode(value, true)
}
pub(crate) fn decode_outcome_result(value: &Value) -> Result<FormResult> {
    decode(value, false)
}
fn decode(value: &Value, reserve_outcome: bool) -> Result<FormResult> {
    let r = record(value, "form result")?;
    // TypeScript's {kind: 'form', ...record} also accepts an explicit form kind.
    if let Some(kind) = r.get("kind") {
        ensure(kind == "form")?;
    }
    match value["action"].as_str() {
        Some("accept") => {
            shaped(r, &["action", "values"], &["kind"])?;
            let values = record(&value["values"], "form values")?;
            ensure(values.len() <= 32)?;
            let values = values
                .iter()
                .map(|(key, v)| {
                    let key = text(&Value::String(key.clone()), 256, false)?;
                    let value = match v {
                        Value::String(_) => FormValue::String(text(v, 2048, true)?),
                        Value::Number(_) => FormValue::Number(number(v)?),
                        Value::Bool(v) => FormValue::Boolean(*v),
                        _ => FormValue::Strings(strings(v)?),
                    };
                    Ok((key, value))
                })
                .collect::<Result<BTreeMap<_, _>>>()?;
            if reserve_outcome {
                validation::answer_budget(&values)?;
            }
            Ok(FormResult::Accept { values })
        }
        Some(action @ ("decline" | "cancel")) => {
            shaped(r, &["action"], &["kind"])?;
            Ok(if action == "decline" {
                FormResult::Decline
            } else {
                FormResult::Cancel
            })
        }
        _ => Err(invalid()),
    }
}

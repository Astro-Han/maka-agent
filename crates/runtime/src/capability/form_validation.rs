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
    form_decode::{ensure, invalid},
    json::encoded_limit,
};
use serde_json::json;
type Result<T> = std::result::Result<T, &'static str>;
use std::collections::BTreeMap;

pub(super) fn default_value(spec: &FormFieldSpec) -> Option<FormValue> {
    match spec {
        FormFieldSpec::String { default, .. } | FormFieldSpec::SingleSelect { default, .. } => {
            default.clone().map(FormValue::String)
        }
        FormFieldSpec::Number { default, .. } | FormFieldSpec::Integer { default, .. } => {
            default.map(FormValue::Number)
        }
        FormFieldSpec::Boolean { default } => default.map(FormValue::Boolean),
        FormFieldSpec::MultiSelect { default, .. } => default.clone().map(FormValue::Strings),
    }
}
pub(super) fn valid(spec: &FormFieldSpec, value: &FormValue) -> bool {
    match (spec, value) {
        (
            FormFieldSpec::String {
                min_length,
                max_length,
                format,
                ..
            },
            FormValue::String(v),
        ) => {
            min_length.is_none_or(|n| v.chars().count() >= n)
                && max_length.is_none_or(|n| v.chars().count() <= n)
                && matches_format(v, *format)
        }
        (
            FormFieldSpec::Number {
                minimum, maximum, ..
            }
            | FormFieldSpec::Integer {
                minimum, maximum, ..
            },
            FormValue::Number(v),
        ) => {
            v.is_finite()
                && minimum.is_none_or(|n| *v >= n)
                && maximum.is_none_or(|n| *v <= n)
                && (!matches!(spec, FormFieldSpec::Integer { .. })
                    || (v.fract() == 0.0 && v.abs() <= 9_007_199_254_740_991.0))
        }
        (FormFieldSpec::Boolean { .. }, FormValue::Boolean(_)) => true,
        (FormFieldSpec::SingleSelect { options, .. }, FormValue::String(v)) => {
            options.iter().any(|o| &o.value == v)
        }
        (
            FormFieldSpec::MultiSelect {
                options,
                min_items,
                max_items,
                ..
            },
            FormValue::Strings(v),
        ) => {
            min_items.is_none_or(|n| v.len() >= n)
                && max_items.is_none_or(|n| v.len() <= n)
                && v.iter()
                    .enumerate()
                    .all(|(i, s)| !v[..i].contains(s) && options.iter().any(|o| &o.value == s))
        }
        _ => false,
    }
}
fn matches_format(value: &str, format: Option<FormFormat>) -> bool {
    let Some(format) = format else { return true };
    match format {
        FormFormat::Uri => url::Url::parse(value).is_ok(),
        FormFormat::Email => regress::Regex::new(r"^[^\s@]+@[^\s@]+\.[^\s@]+$")
            .unwrap()
            .find(value)
            .is_some(),
        FormFormat::Date | FormFormat::DateTime => calendar(value, format == FormFormat::DateTime),
    }
}
fn calendar(value: &str, time: bool) -> bool {
    let pattern = if time {
        r"^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d+)?(?:Z|[+-](\d{2}):(\d{2}))$"
    } else {
        r"^(\d{4})-(\d{2})-(\d{2})$"
    };
    let regex = regress::Regex::new(pattern).unwrap();
    let Some(m) = regex.find(value) else {
        return false;
    };
    let n = |index: usize| {
        m.captures[index]
            .clone()
            .and_then(|r| value[r].parse::<u32>().ok())
            .unwrap_or(0)
    };
    let (year, month, day) = (n(0), n(1), n(2));
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    (1..=12).contains(&month)
        && day >= 1
        && day <= days[(month - 1) as usize]
        && (!time || (n(3) <= 23 && n(4) <= 59 && n(5) <= 59 && n(6) <= 23 && n(7) <= 59))
}
pub(super) fn answer_budget(values: &BTreeMap<String, FormValue>) -> Result<()> {
    encoded_limit(
        &json!({"kind":"form","action":"accept","values":values}),
        8192,
    )?;
    encoded_limit(
        &json!({"kind":"form_answer","action":"accept","values":values,
        "committedAt":9_007_199_254_740_991_u64}),
        8192,
    )
}
pub(super) fn admit(input: &FormInput) -> Result<()> {
    let witness = input
        .fields
        .iter()
        .filter(|f| f.required)
        .map(|f| {
            // The source builds its witness with ordinary object assignment;
            // __proto__ never becomes an own answer property there.
            ensure(f.name != "__proto__")?;
            let value = witness(&f.spec)?;
            ensure(valid(&f.spec, &value))?;
            Ok((f.name.clone(), value))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    answer_budget(&witness)?;
    let envelope = input
        .fields
        .iter()
        .map(|f| (f.name.clone(), envelope(&f.spec)))
        .collect();
    answer_budget(&envelope)
}
fn witness(spec: &FormFieldSpec) -> Result<FormValue> {
    if let Some(value) = default_value(spec) {
        return Ok(value);
    }
    let value = match spec {
        FormFieldSpec::String {
            min_length, format, ..
        } => {
            let min = min_length.unwrap_or(0);
            let value = match format {
                Some(FormFormat::Email) => {
                    format!("{}@b.co", "a".repeat(min.saturating_sub(5).max(1)))
                }
                Some(FormFormat::Uri) => {
                    format!("https://a.co/{}", "a".repeat(min.saturating_sub(13)))
                }
                Some(FormFormat::Date) => "2000-01-01".into(),
                Some(FormFormat::DateTime) => {
                    if min <= 20 {
                        "2000-01-01T00:00:00Z".into()
                    } else {
                        format!(
                            "2000-01-01T00:00:00.{}Z",
                            "0".repeat(min.saturating_sub(21).max(1))
                        )
                    }
                }
                None => "a".repeat(min),
            };
            FormValue::String(value)
        }
        FormFieldSpec::Number {
            minimum, maximum, ..
        }
        | FormFieldSpec::Integer {
            minimum, maximum, ..
        } => {
            let lower = minimum.unwrap_or(f64::NEG_INFINITY);
            let upper = maximum.unwrap_or(f64::INFINITY);
            let integer = matches!(spec, FormFieldSpec::Integer { .. });
            FormValue::Number(if lower > 0.0 {
                if integer { lower.ceil() } else { lower }
            } else if upper < 0.0 {
                if integer { upper.floor() } else { upper }
            } else {
                0.0
            })
        }
        FormFieldSpec::Boolean { .. } => FormValue::Boolean(false),
        FormFieldSpec::SingleSelect { options, .. } => {
            FormValue::String(options.first().ok_or_else(invalid)?.value.clone())
        }
        FormFieldSpec::MultiSelect {
            options, min_items, ..
        } => FormValue::Strings(
            options
                .iter()
                .take(min_items.unwrap_or(0))
                .map(|o| o.value.clone())
                .collect(),
        ),
    };
    Ok(value)
}
fn envelope(spec: &FormFieldSpec) -> FormValue {
    match spec {
        FormFieldSpec::String {
            max_length, format, ..
        } => {
            let max = max_length.unwrap_or(2048).min(2048);
            FormValue::String(match format {
                Some(FormFormat::Date) => "0000-01-01".into(),
                Some(FormFormat::DateTime) => "0".repeat(max),
                _ => "\u{1}".repeat(max),
            })
        }
        FormFieldSpec::Number { .. } | FormFieldSpec::Integer { .. } => {
            FormValue::Number(-f64::MAX)
        }
        FormFieldSpec::Boolean { .. } => FormValue::Boolean(false),
        FormFieldSpec::SingleSelect { options, .. } => FormValue::String(
            options
                .iter()
                .max_by_key(|o| serialized_string_size(&o.value))
                .unwrap()
                .value
                .clone(),
        ),
        FormFieldSpec::MultiSelect {
            options, max_items, ..
        } => {
            let mut values: Vec<_> = options.iter().map(|o| o.value.clone()).collect();
            values.sort_by_key(|v| std::cmp::Reverse(serialized_string_size(v)));
            values.truncate(max_items.unwrap_or(options.len()));
            FormValue::Strings(values)
        }
    }
}
fn serialized_string_size(value: &str) -> usize {
    serde_json::to_string(value).unwrap().len()
}

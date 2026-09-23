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

use super::{Text, VERSION};
use crate::Error;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Finite documents share Remote's payload budget; no executable expressions or nested widgets.
pub const MAX_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub version: u32,
    pub title: Text,
    pub revision: String,
    pub body: String,
    pub rows: Vec<Row>,
    pub fields: Vec<Field>,
    pub actions: Vec<Action>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    pub id: String,
    pub title: Text,
    pub description: String,
    pub route: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub id: String,
    pub label: Text,
    pub enabled: bool,
    pub control: Control,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Control {
    Toggle {
        value: bool,
    },
    Text {
        value: String,
        max_bytes: usize,
        multiline: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub id: String,
    pub label: Text,
    pub enabled: bool,
    /// Only these fields are submitted; unrelated drafts are not implicit input.
    pub fields: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Read {
        route: Value,
    },
    Submit {
        route: Value,
        revision: String,
        action: String,
        fields: BTreeMap<String, Value>,
    },
}

/// A successful write is acknowledged independently of the following page read.
/// Failure to refresh cannot turn a committed write into a retryable submission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Reply {
    Page { page: Page },
    Applied { route: Value },
    Conflict,
    Rejected { message: Text },
}

fn invalid() -> Error {
    Error::Invalid("Invalid terminal page".into())
}
fn bounded(value: &impl Serialize, max: usize) -> Result<(), Error> {
    if serde_json::to_vec(value).map_err(|_| invalid())?.len() > max {
        return Err(invalid());
    }
    Ok(())
}
fn identifier(value: &str) -> Result<(), Error> {
    if value.is_empty() || value.len() > 256 || !safe(value, false) {
        return Err(invalid());
    }
    Ok(())
}
fn safe(value: &str, multiline: bool) -> bool {
    !value.chars().any(|c| {
        (c.is_control() && !(multiline && matches!(c, '\n' | '\t')))
            || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    })
}
fn route(value: &Value) -> Result<(), Error> {
    fn count(value: &Value, depth: usize, nodes: &mut usize) -> Result<(), Error> {
        *nodes += 1;
        if depth > 8 || *nodes > 128 {
            return Err(invalid());
        }
        match value {
            Value::Array(items) => {
                for item in items {
                    count(item, depth + 1, nodes)?;
                }
            }
            Value::Object(items) => {
                for (key, item) in items {
                    if !safe(key, false) {
                        return Err(invalid());
                    }
                    count(item, depth + 1, nodes)?;
                }
            }
            Value::String(text) if !safe(text, false) => return Err(invalid()),
            _ => {}
        }
        Ok(())
    }
    count(value, 0, &mut 0)?;
    bounded(value, 8192)
}

impl Page {
    pub fn validate(&self) -> Result<(), Error> {
        if self.version != VERSION
            || self.rows.len() > 64
            || self.fields.len() > 16
            || self.actions.len() > 16
            || self.body.len() > 32768
            || !safe(&self.body, true)
        {
            return Err(invalid());
        }
        self.title.validate()?;
        identifier(&self.revision)?;
        let mut ids = BTreeSet::new();
        for row in &self.rows {
            identifier(&row.id)?;
            if !ids.insert(&row.id)
                || row.description.len() > 1024
                || !safe(&row.description, false)
            {
                return Err(invalid());
            }
            row.title.validate()?;
            route(&row.route)?;
        }
        let mut fields = BTreeSet::new();
        for field in &self.fields {
            identifier(&field.id)?;
            if !fields.insert(&field.id) {
                return Err(invalid());
            }
            field.label.validate()?;
            if let Control::Text {
                value,
                max_bytes,
                multiline,
            } = &field.control
                && (*max_bytes == 0
                    || *max_bytes > 16384
                    || value.len() > *max_bytes
                    || !safe(value, *multiline))
            {
                return Err(invalid());
            }
        }
        let mut actions = BTreeSet::new();
        for action in &self.actions {
            identifier(&action.id)?;
            action.label.validate()?;
            let mut used = BTreeSet::new();
            if !actions.insert(&action.id)
                || action
                    .fields
                    .iter()
                    .any(|id| !fields.contains(id) || !used.insert(id))
            {
                return Err(invalid());
            }
        }
        bounded(self, MAX_BYTES - 64)
    }

    /// A presentation check, not authorization: the plugin must validate again.
    pub fn submission(
        &self,
        route: Value,
        action: &str,
        fields: BTreeMap<String, Value>,
    ) -> Result<Request, Error> {
        self.validate()?;
        let action = self
            .actions
            .iter()
            .find(|item| item.id == action && item.enabled)
            .ok_or_else(invalid)?;
        if action.fields.len() != fields.len() {
            return Err(invalid());
        }
        for id in &action.fields {
            let field = self
                .fields
                .iter()
                .find(|field| &field.id == id && field.enabled)
                .ok_or_else(invalid)?;
            let value = fields.get(id).ok_or_else(invalid)?;
            match &field.control {
                Control::Toggle { .. } if value.is_boolean() => {}
                Control::Text {
                    max_bytes,
                    multiline,
                    ..
                } if value
                    .as_str()
                    .is_some_and(|text| text.len() <= *max_bytes && safe(text, *multiline)) => {}
                _ => return Err(invalid()),
            }
        }
        let request = Request::Submit {
            route,
            revision: self.revision.clone(),
            action: action.id.clone(),
            fields,
        };
        request.validate()?;
        Ok(request)
    }
}
impl Request {
    pub fn validate(&self) -> Result<(), Error> {
        match self {
            Self::Read { route: value } => route(value)?,
            Self::Submit {
                route: value,
                revision,
                action,
                fields,
            } => {
                route(value)?;
                identifier(revision)?;
                identifier(action)?;
                if fields.len() > 16 {
                    return Err(invalid());
                }
                for (id, value) in fields {
                    identifier(id)?;
                    match value {
                        Value::Bool(_) => {}
                        Value::String(text) if text.len() <= 16384 && safe(text, true) => {}
                        _ => return Err(invalid()),
                    }
                }
            }
        }
        bounded(self, MAX_BYTES)
    }
}
impl Reply {
    pub fn validate(&self) -> Result<(), Error> {
        match self {
            Self::Page { page } => page.validate()?,
            Self::Applied { route: value } => route(value)?,
            Self::Rejected { message } => message.validate()?,
            Self::Conflict => {}
        }
        bounded(self, MAX_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn page() -> Page {
        Page {
            version: VERSION,
            title: Text::plain("Preferences"),
            revision: "r1".into(),
            body: String::new(),
            rows: vec![],
            fields: vec![Field {
                id: "enabled".into(),
                label: Text::plain("Enabled"),
                enabled: true,
                control: Control::Toggle { value: true },
            }],
            actions: vec![Action {
                id: "save".into(),
                label: Text::plain("Save"),
                enabled: true,
                fields: vec!["enabled".into()],
            }],
        }
    }
    #[test]
    fn page_contract_rejects_ambiguous_controls_unsafe_content_and_unbounded_routes() {
        let valid = page();
        valid.validate().unwrap();
        let fields = BTreeMap::from([("enabled".into(), json!(false))]);
        let request = valid
            .submission(json!(null), "save", fields.clone())
            .unwrap();
        request.validate().unwrap();
        for variant in 0..7 {
            let mut invalid = valid.clone();
            match variant {
                0 => invalid.version += 1,
                1 => invalid.fields.push(invalid.fields[0].clone()),
                2 => invalid.actions[0].fields.push("absent".into()),
                3 => invalid.body = "\u{001b}[2J".into(),
                4 => invalid.body = "\u{202e}spoof".into(),
                5 => invalid.body = "x".repeat(32769),
                _ => invalid.actions.push(invalid.actions[0].clone()),
            }
            assert!(invalid.validate().is_err(), "{variant}");
        }
        assert!(valid.submission(json!(null), "other", fields).is_err());
        assert!(
            valid
                .submission(
                    json!(null),
                    "save",
                    BTreeMap::from([("enabled".into(), json!("false"))])
                )
                .is_err()
        );
        let mut deep = json!(null);
        for _ in 0..10 {
            deep = json!([deep]);
        }
        assert!(Request::Read { route: deep }.validate().is_err());
        assert!(
            Request::Read {
                route: json!(vec![Value::Null; 129])
            }
            .validate()
            .is_err()
        );
        let mut wire = serde_json::to_value(valid).unwrap();
        wire["fields"][0]["control"]["kind"] = json!("execute");
        assert!(serde_json::from_value::<Page>(wire).is_err());
    }
}

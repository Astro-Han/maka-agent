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

use crate::Error;
use serde_json::{Map, Number, Value};
use yaml_rust2::{
    Yaml,
    parser::{Event, Parser},
    scanner::TScalarStyle,
};

enum Container {
    Sequence(Vec<Value>),
    Mapping(Map<String, Value>, Option<String>),
}

/// YAML is only a package syntax. Reject aliases, duplicate/non-string keys,
/// custom tags and unbounded nesting before constructing the JSON contract.
pub(super) fn parse(bytes: &[u8]) -> Result<Value, Error> {
    if bytes.len() > 512 * 1024 {
        return Err(invalid());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    parse_text(text).ok_or_else(invalid)
}

fn parse_text(text: &str) -> Option<Value> {
    let mut parser = Parser::new_from_str(text);
    let mut stack = Vec::new();
    let mut root = None;
    let mut documents = 0;
    for _ in 0..131_072 {
        let (event, _) = parser.next_token().ok()?;
        let value = match event {
            Event::Alias(_) => return None,
            Event::DocumentStart => {
                documents += 1;
                if documents > 1 {
                    return None;
                }
                continue;
            }
            Event::SequenceStart(_, tag) => {
                if tag.is_some() || stack.len() >= 64 {
                    return None;
                }
                stack.push(Container::Sequence(Vec::new()));
                continue;
            }
            Event::MappingStart(_, tag) => {
                if tag.is_some() || stack.len() >= 64 {
                    return None;
                }
                stack.push(Container::Mapping(Map::new(), None));
                continue;
            }
            Event::SequenceEnd => {
                let Container::Sequence(items) = stack.pop()? else {
                    return None;
                };
                Value::Array(items)
            }
            Event::MappingEnd => {
                let Container::Mapping(entries, None) = stack.pop()? else {
                    return None;
                };
                Value::Object(entries)
            }
            Event::Scalar(value, style, _, tag) => {
                if tag.is_some() {
                    return None;
                }
                if style == TScalarStyle::Plain {
                    scalar(value)?
                } else {
                    Value::String(value)
                }
            }
            Event::StreamEnd => return stack.is_empty().then_some(root.unwrap_or(Value::Null)),
            _ => continue,
        };
        match stack.last_mut() {
            Some(Container::Sequence(items)) => items.push(value),
            Some(Container::Mapping(entries, key)) => {
                if let Some(key) = key.take() {
                    if entries.insert(key, value).is_some() {
                        return None;
                    }
                } else {
                    let Value::String(value) = value else {
                        return None;
                    };
                    *key = Some(value);
                }
            }
            None => {
                if root.replace(value).is_some() {
                    return None;
                }
            }
        }
    }
    None
}

fn scalar(text: String) -> Option<Value> {
    if matches!(text.as_str(), "null" | "Null" | "NULL" | "~" | "") {
        return Some(Value::Null);
    }
    match Yaml::from_str(&text) {
        Yaml::Null => Some(Value::Null),
        Yaml::Boolean(value) => Some(Value::Bool(value)),
        Yaml::Integer(value) => Some(Value::Number(value.into())),
        Yaml::Real(value) => {
            // Preserve unsigned integers before considering a floating-point value.
            let number = value
                .parse::<u64>()
                .ok()
                .map(Number::from)
                .or_else(|| value.parse::<f64>().ok().and_then(Number::from_f64))?;
            Some(Value::Number(number))
        }
        Yaml::String(value) => Some(Value::String(value)),
        _ => None,
    }
}

fn invalid() -> Error {
    Error::Invalid("composition requires bounded, single-document YAML with JSON values and unique string keys; aliases and tags are unsupported".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn yaml_preserves_multiline_and_rejects_ambiguous_or_expanding_documents() {
        let value = parse(b"- type: insert\n  entry:\n    id: example\n    config:\n      text: |\n        first\n        second\n      count: 2\n      enabled: true\n").unwrap();
        assert_eq!(value[0]["entry"]["config"]["text"], "first\nsecond\n");
        assert_eq!(value[0]["entry"]["config"]["count"], 2);
        for text in [
            "a: 1\na: 2",
            "a: &x [1]\nb: *x",
            "---\na: 1\n---\na: 2",
            "a: !!str 1",
            "a: .nan",
        ] {
            assert!(parse(text.as_bytes()).is_err(), "{text}");
        }
    }
}

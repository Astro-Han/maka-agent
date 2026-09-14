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

use std::{collections::BTreeMap, ops::Index};
use yaml_rust2::{
    parser::{Event, Parser, Tag},
    scanner::TScalarStyle,
};

// Only scalar categories matter to metadata validation. In particular a number
// outside a machine integer's range must never turn into a string requirement.
#[derive(Debug)]
pub(crate) enum Value {
    Null,
    Other,
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

impl Value {
    pub(crate) fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
    pub(crate) fn as_str(&self) -> Option<&str> {
        if let Self::String(text) = self {
            Some(text)
        } else {
            None
        }
    }
    pub(crate) fn as_vec(&self) -> Option<&Vec<Self>> {
        if let Self::Array(items) = self {
            Some(items)
        } else {
            None
        }
    }
    pub(crate) fn as_hash(&self) -> Option<&BTreeMap<String, Self>> {
        if let Self::Object(entries) = self {
            Some(entries)
        } else {
            None
        }
    }
}
impl Index<&str> for Value {
    type Output = Self;
    fn index(&self, key: &str) -> &Self {
        self.as_hash()
            .and_then(|entries| entries.get(key))
            .unwrap_or(&Value::Null)
    }
}

enum Container {
    Sequence(Vec<Value>),
    Mapping {
        entries: BTreeMap<String, Value>,
        key: Option<String>,
    },
}

pub(crate) fn parse(text: &str) -> Option<Value> {
    let mut parser = Parser::new_from_str(text);
    let mut stack = Vec::new();
    let mut root = None;
    let mut documents = 0;
    for _ in 0..32_768 {
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
                container_tag(tag, "seq")?;
                if stack.len() >= 32 {
                    return None;
                }
                stack.push(Container::Sequence(Vec::new()));
                continue;
            }
            Event::MappingStart(_, tag) => {
                container_tag(tag, "map")?;
                if stack.len() >= 32 {
                    return None;
                }
                stack.push(Container::Mapping {
                    entries: BTreeMap::new(),
                    key: None,
                });
                continue;
            }
            Event::SequenceEnd => {
                let Container::Sequence(items) = stack.pop()? else {
                    return None;
                };
                Value::Array(items)
            }
            Event::MappingEnd => {
                let Container::Mapping { entries, key: None } = stack.pop()? else {
                    return None;
                };
                Value::Object(entries)
            }
            Event::Scalar(value, style, _, tag) => scalar(value, style, tag)?,
            Event::StreamEnd => return stack.is_empty().then_some(root.unwrap_or(Value::Null)),
            _ => continue,
        };
        match stack.last_mut() {
            Some(Container::Sequence(items)) => items.push(value),
            Some(Container::Mapping { entries, key }) => {
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

fn container_tag(tag: Option<Tag>, expected: &str) -> Option<()> {
    if tag.is_some_and(|tag| tag.handle != "tag:yaml.org,2002:" || tag.suffix != expected) {
        None
    } else {
        Some(())
    }
}

fn scalar(value: String, style: TScalarStyle, tag: Option<Tag>) -> Option<Value> {
    let null = matches!(value.as_str(), "" | "~" | "null" | "Null" | "NULL");
    let boolean = matches!(
        value.as_str(),
        "true" | "True" | "TRUE" | "false" | "False" | "FALSE"
    );
    if let Some(tag) = tag {
        if tag.handle != "tag:yaml.org,2002:" {
            return None;
        }
        return match tag.suffix.as_str() {
            "str" => Some(Value::String(value)),
            "null" if null => Some(Value::Null),
            "bool" if boolean => Some(Value::Other),
            "int" if integer(&value) => Some(Value::Other),
            "float" if float(&value) => Some(Value::Other),
            _ => None,
        };
    }
    Some(if style != TScalarStyle::Plain {
        Value::String(value)
    } else if null {
        Value::Null
    } else if boolean || integer(&value) || float(&value) {
        Value::Other
    } else {
        Value::String(value)
    })
}

fn integer(text: &str) -> bool {
    if let Some(hex) = text.strip_prefix("0x") {
        return !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit());
    }
    if let Some(octal) = text.strip_prefix("0o") {
        return !octal.is_empty() && octal.bytes().all(|b| matches!(b, b'0'..=b'7'));
    }
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

fn float(text: &str) -> bool {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    matches!(unsigned, ".inf" | ".Inf" | ".INF")
        || matches!(text, ".nan" | ".NaN" | ".NAN")
        || (text.contains(['.', 'e', 'E'])
            && text.bytes().any(|b| b.is_ascii_digit())
            && text.parse::<f64>().is_ok())
}

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

//! The Client Capability schema subset, not a general JSON Schema metaschema.
use crate::{ProtocolError, Result, codec};
use serde_json::Value;
use std::collections::HashSet;

fn invalid() -> ProtocolError {
    ProtocolError::invalid("Invalid Client Capability tool input schema")
}

pub fn validate(value: &Value) -> Result<()> {
    super::validate_json(value)?;
    super::encoded_limit(value, 32 * 1024)?;
    if value.get("type").and_then(Value::as_str) != Some("object") {
        return Err(invalid());
    }
    let mut references = Vec::new();
    visit(value, &mut references)?;
    for reference in references {
        resolve_reference(value, reference)?;
    }
    Ok(())
}

fn visit<'a>(value: &'a Value, references: &mut Vec<&'a str>) -> Result<()> {
    if value.is_boolean() {
        return Ok(());
    }
    let schema = value.as_object().ok_or_else(invalid)?;
    for (key, value) in schema {
        match key.as_str() {
            "type" => validate_type(value)?,
            "title" | "description" | "format" => {
                value.as_str().ok_or_else(invalid)?;
            }
            "pattern" => validate_pattern(value.as_str().ok_or_else(invalid)?)?,
            "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" | "multipleOf" => {
                let number = value
                    .as_f64()
                    .filter(|n| n.is_finite())
                    .ok_or_else(invalid)?;
                if key == "multipleOf" && number <= 0.0 {
                    return Err(invalid());
                }
            }
            "minItems" | "maxItems" | "minLength" | "maxLength" | "minProperties"
            | "maxProperties" => {
                codec::count(value, key)?;
            }
            "uniqueItems" => {
                value.as_bool().ok_or_else(invalid)?;
            }
            "required" => {
                unique_strings(value.as_array().ok_or_else(invalid)?)?;
            }
            "properties" | "patternProperties" | "$defs" | "definitions" => {
                for (name, nested) in value.as_object().ok_or_else(invalid)? {
                    if key == "patternProperties" {
                        validate_pattern(name)?;
                    }
                    visit(nested, references)?;
                }
            }
            "allOf" | "anyOf" | "oneOf" => visit_array(value, references)?,
            "items" if value.is_array() => visit_array(value, references)?,
            "items" | "additionalItems" | "additionalProperties" | "propertyNames" => {
                visit(value, references)?;
            }
            "enum" => {
                if value.as_array().is_none_or(Vec::is_empty) {
                    return Err(invalid());
                }
            }
            "examples" => {
                value.as_array().ok_or_else(invalid)?;
            }
            "$ref" => references.push(value.as_str().ok_or_else(invalid)?),
            "const" | "default" => {}
            _ => return Err(invalid()),
        }
    }
    Ok(())
}

fn visit_array<'a>(value: &'a Value, references: &mut Vec<&'a str>) -> Result<()> {
    let values = value
        .as_array()
        .filter(|v| !v.is_empty())
        .ok_or_else(invalid)?;
    for nested in values {
        visit(nested, references)?;
    }
    Ok(())
}

fn unique_strings(values: &[Value]) -> Result<HashSet<&str>> {
    let mut seen = HashSet::new();
    for value in values {
        if !seen.insert(value.as_str().ok_or_else(invalid)?) {
            return Err(invalid());
        }
    }
    Ok(seen)
}

fn validate_type(value: &Value) -> Result<()> {
    let values = value
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(std::slice::from_ref(value));
    let types = unique_strings(values)?;
    if types.is_empty()
        || types.iter().any(|v| {
            !matches!(
                *v,
                "array" | "boolean" | "integer" | "null" | "number" | "object" | "string"
            )
        })
    {
        return Err(invalid());
    }
    Ok(())
}

fn resolve_reference(root: &Value, reference: &str) -> Result<()> {
    let mut tokens = reference.strip_prefix("#/").ok_or_else(invalid)?.split('/');
    let namespace = tokens.next().ok_or_else(invalid)?;
    if !matches!(namespace, "$defs" | "definitions") {
        return Err(invalid());
    }
    let mut value = root.get(namespace).ok_or_else(invalid)?;
    let mut has_token = false;
    for token in tokens {
        has_token = true;
        if token.is_empty() {
            return Err(invalid());
        }
        let mut decoded = String::new();
        let mut chars = token.chars();
        while let Some(ch) = chars.next() {
            decoded.push(if ch == '~' {
                match chars.next() {
                    Some('0') => '~',
                    Some('1') => '/',
                    _ => return Err(invalid()),
                }
            } else {
                ch
            });
        }
        // The source forbids traversing arrays, but accepts an array as the final target.
        value = value
            .as_object()
            .and_then(|v| v.get(&decoded))
            .ok_or_else(invalid)?;
    }
    if !has_token || !(value.is_boolean() || value.is_object() || value.is_array()) {
        return Err(invalid());
    }
    Ok(())
}

fn validate_pattern(pattern: &str) -> Result<()> {
    // Deliberately narrower than TypeScript new RegExp: cap structural operators
    // at 128 to bound recursive parsing, lookbehind IR traversal and binary Alt
    // destruction. Disable optimization to avoid expanding counted repetitions.
    // Only compile syntax; never execute these untrusted patterns.
    let mut operators = 0_usize;
    let mut escaped = false;
    let mut in_class = false;
    for ch in pattern.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            '(' | '|' if !in_class => {
                operators += 1;
                if operators > 128 {
                    return Err(invalid());
                }
            }
            _ => {}
        }
    }
    regress::Regex::with_flags(
        pattern,
        regress::Flags {
            no_opt: true,
            ..Default::default()
        },
    )
    .map(|_| ())
    .map_err(|_| invalid())
}

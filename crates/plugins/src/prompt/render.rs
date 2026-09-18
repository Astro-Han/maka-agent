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
use std::collections::BTreeMap;

pub(super) fn interpolate(
    template: &str,
    variables: &BTreeMap<String, Option<String>>,
) -> Result<String, Error> {
    let mut output = String::new();
    let mut rest = template;
    while let Some((prefix, suffix)) = rest.split_once("{{") {
        append(&mut output, prefix)?;
        let Some((name, suffix)) = suffix.split_once("}}") else {
            return Err(Error::Invalid("unterminated prompt variable".into()));
        };
        let name = name.trim();
        let value = variables
            .get(name)
            .ok_or_else(|| Error::Invalid(format!("unknown prompt variable {name:?}")))?;
        let value = value
            .as_deref()
            .ok_or_else(|| Error::Invalid(format!("prompt variable {name:?} has no value")))?;
        append(&mut output, value)?;
        rest = suffix;
    }
    append(&mut output, rest)?;
    Ok(output)
}

pub(super) fn join(values: impl Iterator<Item = String>) -> Result<Option<String>, Error> {
    let mut output = String::new();
    for value in values.filter(|value| !value.is_empty()) {
        if !output.is_empty() {
            append(&mut output, "\n\n")?;
        }
        append(&mut output, &value)?;
    }
    Ok((!output.is_empty()).then_some(output))
}

fn append(output: &mut String, value: &str) -> Result<(), Error> {
    if output.len() + value.len() > 64 * 1024 {
        return Err(Error::Invalid("rendered prompt exceeds 64 KiB".into()));
    }
    output.push_str(value);
    Ok(())
}

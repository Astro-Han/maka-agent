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

use crate::{Operation, ProtocolError, Result, codec::MAX_SAFE_INTEGER};
use maka_runtime::pricing::{Mutation, Page, Query, Update, Updated};
use serde_json::Value;

pub fn supports(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::PricingQuery | Operation::PricingMutate
    )
}

pub enum Input {
    Query(Query),
    Update(Update),
}

pub fn decode_input(operation: Operation, value: &Value) -> Result<Input> {
    match operation {
        Operation::PricingQuery => {
            let query: Query = decode(value)?;
            if let Query::Continue { revision, offset } = query
                && (revision > MAX_SAFE_INTEGER || offset > MAX_SAFE_INTEGER)
            {
                return Err(ProtocolError::invalid("invalid pricing continuation"));
            }
            Ok(Input::Query(query))
        }
        Operation::PricingMutate => {
            let input: Update = decode(value)?;
            if input.expected_revision > MAX_SAFE_INTEGER {
                return Err(ProtocolError::invalid("invalid pricing revision"));
            }
            match &input.mutation {
                Mutation::Upsert { pricing } => pricing.validate(),
                Mutation::Delete { model_key } => maka_runtime::pricing::validate_key(model_key),
            }
            .map_err(ProtocolError::invalid)?;
            Ok(Input::Update(input))
        }
        _ => Err(ProtocolError::invalid("unknown pricing operation")),
    }
}

pub fn decode_output(operation: Operation, value: &Value) -> Result<Value> {
    match operation {
        Operation::PricingQuery => {
            let result: Page = decode(value)?;
            match result {
                Page::RevisionChanged {
                    expected_revision,
                    actual_revision,
                } => {
                    conflict(expected_revision, actual_revision)?;
                }
                Page::Page {
                    revision,
                    offset,
                    entries,
                    next_offset,
                } => {
                    if revision > MAX_SAFE_INTEGER
                        || offset > MAX_SAFE_INTEGER
                        || entries.len() > 128
                        || serde_json::to_vec(value).map_err(invalid)?.len() > 48 * 1024
                        || next_offset.is_some_and(|next| {
                            entries.is_empty()
                                || next > MAX_SAFE_INTEGER
                                || next != offset + entries.len() as u64
                        })
                    {
                        return Err(ProtocolError::invalid("invalid pricing page"));
                    }
                    let mut previous: Option<&str> = None;
                    for entry in &entries {
                        let (maka_runtime::pricing::Entry::Builtin { pricing }
                        | maka_runtime::pricing::Entry::Custom { pricing, .. }) = entry;
                        pricing.validate().map_err(ProtocolError::invalid)?;
                        if previous.is_some_and(|key| {
                            key.encode_utf16()
                                .cmp(pricing.model_key.encode_utf16())
                                .is_ge()
                        }) {
                            return Err(ProtocolError::invalid("pricing page is not ordered"));
                        }
                        previous = Some(&pricing.model_key);
                    }
                }
            }
        }
        Operation::PricingMutate => match decode::<Updated>(value)? {
            Updated::Committed { revision } | Updated::Unchanged { revision } => {
                if revision > MAX_SAFE_INTEGER {
                    return Err(ProtocolError::invalid("invalid pricing revision"));
                }
            }
            Updated::RevisionConflict {
                expected_revision,
                actual_revision,
            } => conflict(expected_revision, actual_revision)?,
        },
        _ => return Err(ProtocolError::invalid("unknown pricing operation")),
    }
    Ok(value.clone())
}

fn conflict(expected: u64, actual: u64) -> Result<()> {
    if expected > MAX_SAFE_INTEGER || actual > MAX_SAFE_INTEGER || expected == actual {
        return Err(ProtocolError::invalid("invalid pricing revision conflict"));
    }
    Ok(())
}
fn decode<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T> {
    let mut value = value.clone();
    for field in [
        "revision",
        "offset",
        "expectedRevision",
        "actualRevision",
        "nextOffset",
    ] {
        if let Some(number) = value.get_mut(field)
            && !number.is_null()
        {
            *number = crate::codec::count(number, field)?.into();
        }
    }
    serde_json::from_value(value).map_err(invalid)
}
fn invalid(error: impl std::fmt::Display) -> ProtocolError {
    ProtocolError::invalid(error.to_string())
}

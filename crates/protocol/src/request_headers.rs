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

use crate::{ProtocolError, Result};
use maka_runtime::configuration::{headers::*, validation};
use serde::de::DeserializeOwned;
use serde_json::Value;

fn decode<T: DeserializeOwned>(value: &Value) -> Result<T> {
    serde_json::from_value(value.clone())
        .map_err(|_| ProtocolError::invalid("Invalid request headers payload"))
}

pub fn decode_query(value: &Value) -> Result<RequestHeadersQuery> {
    let input: RequestHeadersQuery = decode(value)?;
    validation::entity_id(&input.connection_id).map_err(ProtocolError::invalid)?;
    Ok(input)
}

pub fn decode_replace(value: &Value) -> Result<RequestHeadersReplace> {
    let mut input: RequestHeadersReplace = decode(value)?;
    input.normalize().map_err(ProtocolError::invalid)?;
    Ok(input)
}

fn normalize_names(names: &mut Vec<String>) -> Result<()> {
    let mut updates = names
        .iter()
        .map(|name| RequestHeaderUpdate {
            name: name.clone(),
            value: None,
        })
        .collect::<Vec<_>>();
    normalize_updates(&mut updates).map_err(ProtocolError::invalid)?;
    *names = updates.into_iter().map(|update| update.name).collect();
    Ok(())
}

pub fn decode_query_result(value: &Value) -> Result<RequestHeadersQueryResult> {
    result_fields(value)?;
    let mut result = decode(value)?;
    if let RequestHeadersQueryResult::Found { names } = &mut result {
        normalize_names(names)?;
    }
    Ok(result)
}

pub fn decode_replace_result(value: &Value) -> Result<RequestHeadersReplaceResult> {
    result_fields(value)?;
    let mut result = decode(value)?;
    match &mut result {
        RequestHeadersReplaceResult::Committed { names }
        | RequestHeadersReplaceResult::Unchanged { names } => normalize_names(names)?,
        RequestHeadersReplaceResult::ConnectionNotFound => {}
    }
    Ok(result)
}

fn result_fields(value: &Value) -> Result<()> {
    let record = crate::codec::record(value, "request headers result")?;
    crate::codec::exact(
        record,
        if value["kind"] == "connection_not_found" {
            &["kind"]
        } else {
            &["kind", "names"]
        },
    )
}

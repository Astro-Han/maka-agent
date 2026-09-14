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

// The wire uses boolean tags; internally a preview is an ordinary Rust Result.
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{DeserializeOwned, Error},
};
use serde_json::Value;

pub fn serialize<T: Serialize, E: Serialize, S: Serializer>(
    value: &Result<T, E>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Success<'a, T> {
        ok: bool,
        #[serde(flatten)]
        value: &'a T,
    }
    #[derive(Serialize)]
    struct Failure<'a, E> {
        ok: bool,
        reason: &'a E,
    }
    match value {
        Ok(value) => Success { ok: true, value }.serialize(serializer),
        Err(reason) => Failure { ok: false, reason }.serialize(serializer),
    }
}

pub fn deserialize<'de, T: DeserializeOwned, E: DeserializeOwned, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Result<T, E>, D::Error> {
    let mut value = serde_json::Map::<String, Value>::deserialize(deserializer)?;
    match value.remove("ok") {
        Some(Value::Bool(true)) => serde_json::from_value(Value::Object(value))
            .map(Ok)
            .map_err(D::Error::custom),
        Some(Value::Bool(false)) if value.len() == 1 && value.contains_key("reason") => {
            serde_json::from_value(value.remove("reason").unwrap())
                .map(Err)
                .map_err(D::Error::custom)
        }
        _ => Err(D::Error::custom("Invalid artifact preview outcome")),
    }
}

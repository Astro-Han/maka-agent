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

use serde::Serialize;
use serde_json::{Value, ser::Formatter};
use std::io;

/// Compact ECMAScript number spellings keep physical interaction records inside
/// the same byte bounds used by the client contract.
pub fn stringify(value: &impl Serialize) -> Result<String, serde_json::Error> {
    struct JsFormatter;
    impl Formatter for JsFormatter {
        fn write_f64<W: ?Sized + io::Write>(
            &mut self,
            writer: &mut W,
            value: f64,
        ) -> io::Result<()> {
            writer.write_all(ryu_js::Buffer::new().format(value).as_bytes())
        }
        fn write_u64<W: ?Sized + io::Write>(
            &mut self,
            writer: &mut W,
            value: u64,
        ) -> io::Result<()> {
            self.write_f64(writer, value as f64)
        }
        fn write_i64<W: ?Sized + io::Write>(
            &mut self,
            writer: &mut W,
            value: i64,
        ) -> io::Result<()> {
            self.write_f64(writer, value as f64)
        }
    }
    let value: Value = serde_json::to_value(value)?;
    let mut bytes = Vec::new();
    value.serialize(&mut serde_json::Serializer::with_formatter(
        &mut bytes,
        JsFormatter,
    ))?;
    Ok(String::from_utf8(bytes).expect("JSON serialization emits UTF-8"))
}
pub fn encoded_limit(value: &impl Serialize, limit: usize) -> Result<(), &'static str> {
    if stringify(value).map_err(|_| "Invalid JSON value")?.len() > limit {
        return Err("Client Capability JSON exceeds byte limit");
    }
    Ok(())
}

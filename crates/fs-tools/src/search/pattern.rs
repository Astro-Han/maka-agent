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

use crate::failed;
use glob::Pattern;
use maka_runtime::tools::ToolError;
use std::path::Path;

pub(super) enum Segment {
    Recursive,
    Literal(String),
    Match(Pattern),
    Directory,
}

pub(super) fn compile(pattern: &str) -> Result<Vec<Segment>, ToolError> {
    if pattern.is_empty()
        || pattern.len() > 4096
        || pattern.contains('\0')
        || Path::new(pattern).is_absolute()
        || pattern.split('/').any(|part| part == "..")
    {
        return Err(failed(
            "Glob requires a bounded relative pattern without parent traversal; select the search directory with cwd",
        ));
    }
    if pattern.contains(['{', '}', '\\'])
        || ["?(", "*(", "+(", "@(", "!("]
            .iter()
            .any(|part| pattern.contains(part))
    {
        return Err(failed(
            "Glob currently supports Unix wildcards, not brace expansion, escapes or extglobs",
        ));
    }
    let mut segments = Vec::new();
    for part in pattern
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
    {
        if part == "**" {
            if !matches!(segments.last(), Some(Segment::Recursive)) {
                segments.push(Segment::Recursive);
            }
        } else if !part.contains(['*', '?', '[']) {
            segments.push(Segment::Literal(part.to_owned()));
        } else {
            segments.push(Segment::Match(
                Pattern::new(part).map_err(|e| failed(format!("Glob pattern: {e}")))?,
            ));
        }
    }
    if !segments.is_empty() && pattern.ends_with('/') {
        segments.push(Segment::Directory);
    }
    if segments.len() > 128 {
        return Err(failed("Glob pattern exceeds path depth limit"));
    }
    Ok(segments)
}

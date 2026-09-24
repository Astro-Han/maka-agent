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

//! How a tool call reads in one line: an icon, a verb and what it acted on.

use super::rows::Entry;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Think,
    Run,
    Edit,
    Read,
    Search,
    Plan,
    Ask,
    Tool,
}

impl Kind {
    pub fn of(entry: &Entry) -> Self {
        let Entry::Tool { name, .. } = entry else {
            return Self::Think;
        };
        match name.as_str() {
            "Shell" | "Bash" | "exec" | "shell" => Self::Run,
            "apply_patch" | "Edit" | "Write" | "MultiEdit" => Self::Edit,
            "Read" | "WebFetch" => Self::Read,
            "Glob" | "Grep" | "WebSearch" | "web_search" | "tool_search" => Self::Search,
            "TodoWrite" | "update_plan" => Self::Plan,
            "AskUserQuestion" => Self::Ask,
            _ => Self::Tool,
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Think => "icons/sparkles.svg",
            Self::Run => "icons/square-terminal.svg",
            Self::Edit => "icons/pencil.svg",
            Self::Read => "icons/file-text.svg",
            Self::Search => "icons/search.svg",
            Self::Plan => "icons/list.svg",
            Self::Ask => "icons/message-circle-question-mark.svg",
            Self::Tool => "icons/wrench.svg",
        }
    }

    pub fn verb(self) -> &'static str {
        match self {
            Self::Think => "思考",
            Self::Run => "运行",
            Self::Edit => "编辑",
            Self::Read => "读取",
            Self::Search => "搜索",
            Self::Plan => "计划",
            Self::Ask => "提问",
            Self::Tool => "工具",
        }
    }

    fn counted(self, count: usize) -> String {
        match self {
            Self::Think => format!("{count} 段思考"),
            Self::Run => format!("{count} 条命令"),
            Self::Edit => format!("{count} 处编辑"),
            Self::Read => format!("{count} 次读取"),
            Self::Search => format!("{count} 次搜索"),
            Self::Plan => format!("{count} 次计划"),
            Self::Ask => format!("{count} 个问题"),
            Self::Tool => format!("{count} 次工具调用"),
        }
    }
}

/// What the call acted on, in a few words.
pub fn target(entry: &Entry) -> String {
    let Entry::Tool { name, args, .. } = entry else {
        return String::new();
    };
    let field = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| args[*key].as_str())
            .map(str::to_owned)
    };
    let text = match Kind::of(entry) {
        Kind::Run => field(&["description", "command", "cmd"]),
        Kind::Edit | Kind::Read => field(&["path", "file_path", "url"])
            .or_else(|| args["operation"]["path"].as_str().map(str::to_owned))
            .map(|path| file_name(&path)),
        Kind::Search => field(&["pattern", "query"]),
        Kind::Tool => Some(humanize(name)),
        _ => None,
    };
    first_line(&text.unwrap_or_default())
}

/// Summary of a run of calls: counts per kind in the order they appear.
pub fn summary(entries: &[&Entry], running: bool) -> String {
    let mut counts: Vec<(Kind, usize)> = Vec::new();
    for entry in entries {
        let kind = Kind::of(entry);
        match counts.iter_mut().find(|(seen, _)| *seen == kind) {
            Some((_, count)) => *count += 1,
            None => counts.push((kind, 1)),
        }
    }
    let parts: Vec<String> = counts
        .into_iter()
        .map(|(kind, count)| kind.counted(count))
        .collect();
    format!(
        "{}{}",
        if running {
            "正在执行 "
        } else {
            "已执行 "
        },
        parts.join(" · ")
    )
}

/// The arguments worth showing when a call is expanded.
pub fn input(entry: &Entry) -> Option<String> {
    let Entry::Tool { args, .. } = entry else {
        return None;
    };
    if let Some(command) = args["command"].as_str() {
        return Some(command.to_owned());
    }
    match args {
        Value::Object(map) if map.is_empty() => None,
        Value::Null => None,
        _ => serde_json::to_string_pretty(args).ok(),
    }
}

fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(path)
        .to_owned()
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    line.trim().to_owned()
}

/// `mcp__github__list_issues` reads as "List issues".
fn humanize(name: &str) -> String {
    if name.contains(' ') {
        return name.to_owned();
    }
    let leaf = name.rsplit([':', '.', '/']).next().unwrap_or(name);
    let leaf = leaf.rsplit("__").next().unwrap_or(leaf);
    let mut words = String::new();
    let mut previous_lower = false;
    for c in leaf.chars() {
        if c == '_' || c == '-' {
            words.push(' ');
            previous_lower = false;
            continue;
        }
        if c.is_uppercase() && previous_lower {
            words.push(' ');
        }
        previous_lower = c.is_lowercase();
        words.extend(c.to_lowercase());
    }
    let mut chars = words.trim().chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str, args: Value) -> Entry {
        Entry::Tool {
            id: "c".into(),
            turn: "t".into(),
            name: name.into(),
            args,
            result: None,
        }
    }

    #[test]
    fn a_call_reads_as_verb_and_target() {
        assert_eq!(
            target(&tool("Shell", json!({"command": "cargo test\n--all"}))),
            "cargo test"
        );
        assert_eq!(
            target(&tool("Read", json!({"path": "src/main.rs"}))),
            "main.rs"
        );
        assert_eq!(
            target(&tool("mcp__github__list_issues", json!({}))),
            "List issues"
        );
        assert_eq!(target(&tool("fetchPage", json!({}))), "Fetch page");
        assert_eq!(Kind::of(&tool("Grep", json!({}))).verb(), "搜索");
    }

    #[test]
    fn a_group_summarizes_by_kind_in_order() {
        let calls = [
            tool("Shell", json!({})),
            tool("Read", json!({})),
            tool("Shell", json!({})),
        ];
        let refs: Vec<&Entry> = calls.iter().collect();
        assert_eq!(summary(&refs, false), "已执行 2 条命令 · 1 次读取");
    }
}

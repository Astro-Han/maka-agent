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

use cap_fs_ext::MetadataExt;
use cap_std::fs::File;
use maka_fs_tools::{EditMatchStrategy, MutationExecutor, WriteCoordinator, WriteScope};
use maka_runtime::tools::ToolExecutor;
use serde_json::{Value, json};
use std::{fs, sync::Arc};
use std::{
    io::Write,
    process::{Command, Stdio},
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn differential_original_typescript_matcher() {
    // Executes the original source, including its upstream attribution. This
    // tests content, winning strategy, line bounds, and success/refusal parity.
    let mut cases: Vec<[String; 3]> = [
        ("aaaaa", "aaa"),           // exact occurrences are non-overlapping
        ("'a'a'a'", "\\'a\\'a\\'"), // fuzzy overlaps are ambiguous
        ("aaaaaaa", "aaaaa\\n"),
        ("  alpha\n  beta\n", "alpha\nbeta\n"),
        ("  alpha\n  beta", "alpha\nbeta\n"),
        ("alpha   beta\ngamma", "alpha beta"),
        ("alpha beta", "alpha\nbeta"),
        ("alpha   beta\n gamma", "alpha beta\ngamma"),
        ("alpha   beta\n", "alpha beta\n"),
        ("xx alpha\nbeta yy", "alpha\\nbeta"),
        ("alpha\\tbeta", "alpha\tbeta"),
        ("alpha\\\nbeta", "alpha\nbeta"),
        ("alpha\\\nbeta", "alpha\\\\"),
        ("alpha\\\nbeta", "alpha\\\\\nbeta"),
        ("  alpha\n alpha", "alpha "),
        ("  alpha\n  alpha", "alpha "),
        ("aaaaaa", "aaaaa\\"),
        ("  ababababa", "ababa\\"),
        ("\u{feff}alpha\u{feff}", " alpha "),
        ("\u{0085}alpha\u{0085}", " alpha "),
        ("\u{200b}alpha\u{200b}", " alpha "),
        ("  😀😀a  ", "😀😀a "),
        ("  😀😀  ", "😀😀 "),
        ("nul\0 alpha", "alpha"),
        ("nul\0 alpha", " alpha "),
        ("\n", "\n"),
        ("\\'\\\"\\`\\$\\\\", "'\"`$\\"),
    ]
    .into_iter()
    .map(|(s, o)| [s.into(), o.into(), "$&$1\n literal".into()])
    .collect();
    for size in [49_999, 50_000] {
        cases.push([
            format!("{}  alpha", "\n".repeat(size)),
            "alpha ".into(),
            "x".into(),
        ]);
    }
    for size in [999_992, 999_993] {
        cases.push([
            format!("{}\n  alpha", "x".repeat(size)),
            "alpha ".into(),
            "x".into(),
        ]);
    }
    cases.push([
        format!("alpha{}beta\ngamma", " ".repeat(600)),
        "alpha beta\ngamma".into(),
        "x".into(),
    ]);
    cases.push([
        "alpha\nbeta\ngamma\ndelta".into(),
        "alpha\\nbeta\\ngamma\\ndelta".into(),
        "x".into(),
    ]);
    cases.push([
        format!("{}\n alpha", "😀".repeat(260_000)),
        "alpha ".into(),
        "x".into(),
    ]);
    // Deterministic combinations exercise interacting boundaries, blank lines,
    // escapes and normalization, rather than duplicating the Rust algorithm.
    let atoms = [
        "alpha",
        " beta ",
        "alpha  beta",
        "alpha\\n",
        "alpha\\",
        "",
        "\u{feff}alpha",
        "alpha\tbeta",
        "alpha\\tbeta",
    ];
    for a in atoms {
        for b in atoms {
            for old in atoms {
                cases.push([format!("{a}\n{b}"), old.into(), "replacement".into()]);
                cases.push([
                    format!("{a}\n{b}\n"),
                    format!("{old}\n beta"),
                    "replacement".into(),
                ]);
            }
        }
    }
    let script = r#"
const fs = require('node:fs');
const esbuild = require('esbuild');
const code = esbuild.transformSync(fs.readFileSync(process.argv[1], 'utf8'), {loader:'ts',format:'cjs'}).code;
const moduleValue = {exports:{}};
new Function('module','exports',code)(moduleValue,moduleValue.exports);
const cases = JSON.parse(fs.readFileSync(0,'utf8'));
process.stdout.write(JSON.stringify(cases.map(([s,o,n]) => {
 try { return moduleValue.exports.computeEditedSource(s,o,n,'fixture'); }
 catch { return null; }
})));
"#;
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/runtime/src/edit-replace.ts");
    let mut child = Command::new("node")
        .arg("-e")
        .arg(script)
        .arg(source)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let encoded = serde_json::to_vec(&cases).unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(&encoded).unwrap());
    let output = child.wait_with_output().unwrap();
    writer.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    #[cfg(windows)]
    let root = std::path::PathBuf::from(root.to_str().unwrap().strip_prefix(r"\\?\").unwrap());
    let executor = MutationExecutor::new(
        &root,
        WriteScope::Restricted {
            roots: vec![root.clone()],
        },
        Arc::new(WriteCoordinator::default()),
    )
    .unwrap();
    let path = root.join("fixture");
    for (i, ([source, old, new], expected)) in cases.iter().zip(expected).enumerate() {
        fs::write(&path, source).unwrap();
        let before = File::from_std(fs::File::open(&path).unwrap())
            .metadata()
            .unwrap();
        let result = executor
            .invoke(
                "Edit".into(),
                json!({"path":"fixture","old_string":old,"new_string":new}),
                CancellationToken::new(),
            )
            .await;
        let content = fs::read_to_string(&path).unwrap();
        let after = File::from_std(fs::File::open(&path).unwrap())
            .metadata()
            .unwrap();
        assert_eq!(
            (after.dev(), after.ino(), after.permissions()),
            (before.dev(), before.ino(), before.permissions()),
            "case {i} metadata"
        );
        let actual = match result {
            Ok(result) => {
                assert_eq!(result["path"], json!(path));
                assert_eq!(result["ok"], true);
                assert_eq!(result["replacements"], 1);
                json!({"content":content, "matchedVia":result["matchedVia"], "startLine":result["startLine"], "endLine":result["endLine"]})
            }
            Err(_) => {
                assert_eq!(&content, source, "case {i} refused edit changed file");
                Value::Null
            }
        };
        assert_eq!(
            actual,
            expected,
            "case {i}: old={old:?}, source prefix={:?}",
            source.chars().take(80).collect::<String>()
        );
    }
    for name in ["exact", "line-trimmed", "whitespace", "escape"] {
        let strategy: EditMatchStrategy = serde_json::from_value(json!(name)).unwrap();
        assert_eq!(serde_json::to_value(strategy).unwrap(), json!(name));
    }
    assert!(serde_json::from_value::<EditMatchStrategy>(json!("approximate")).is_err());
}

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

use super::*;

#[test]
fn visual_custom_theme_saves_reopens_and_rejects_external_changes_without_losing_selection() {
    let directory = tempfile::tempdir().unwrap();
    let mut host = super::super::candidate::CandidateFixture::new(directory.path().join("root"));
    host.child = Some(
        Command::new(env!("CARGO_BIN_EXE_maka"))
            .args(["host", "serve", "--root"])
            .arg(&host.root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    host.wait_for_registration();
    let path = directory.path().join("tui-state/theme.json");
    let mut tui = Pty::spawn(&["--root", host.root.to_str().unwrap()]);
    tui.wait_for("No sessions yet.");
    tui.click_text("Settings");
    tui.wait_for("Customize theme");
    tui.click_text("Customize theme");
    tui.wait_for("Preset colors");
    // Starts on the role list; Tab goes to swatches, then the explicit hex field.
    tui.send(b"\t\t\x01#89ABCD");
    tui.wait_for("#89ABCD");
    tui.click_text("Save & apply");
    tui.wait_for("Palette: My theme");
    let saved = std::fs::read(&path).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&saved).unwrap();
    assert_eq!(json["colors"]["accent"], "#89abcd");
    tui.close_terminal();
    tui.finish();

    let mut reopened = Pty::spawn(&["--root", host.root.to_str().unwrap()]);
    reopened.wait_for("Palette: My theme");
    reopened.click_text("Customize theme");
    reopened.wait_for("Preset colors");
    reopened.wait_for("#89ABCD");
    let external = br##"{"version":1,"name":"External","colors":{"accent":"#123456"}}"##;
    std::fs::write(&path, external).unwrap();
    reopened.click_text("Save & apply");
    reopened.wait_for("File changed externally");
    assert_eq!(std::fs::read(&path).unwrap(), external);
    reopened.click_text("Load custom theme");
    reopened.wait_for("#123456");
    reopened.click_text("Save & apply");
    reopened.wait_for("Palette: External");
    reopened.close_terminal();
    reopened.finish();

    std::fs::write(&path, b"not-json").unwrap();
    let mut invalid = Pty::spawn(&["--root", host.root.to_str().unwrap()]);
    invalid.wait_for("custom unavailable");
    invalid.wait_for("Invalid theme");
    invalid.click_text("◐");
    invalid.wait_for("Palette: Maka dark");
    invalid.close_terminal();
    invalid.finish();
    assert_eq!(std::fs::read(&path).unwrap(), b"not-json");
    host.retire_registered();
}

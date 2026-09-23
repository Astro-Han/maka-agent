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

mod chat;
mod host;
mod workspace;

use gpui_kit::{component::Root, *};
use maka_event_log::root::RootNamespaces;
use std::path::PathBuf;

fn default_root() -> Result<PathBuf, String> {
    let namespaces = RootNamespaces::for_current_account().map_err(|error| error.to_string())?;
    namespaces
        .ownership
        .parent()
        .map(|parent| parent.join("runtime-host-rust"))
        .ok_or_else(|| "missing account data directory".into())
}

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let root = match (args.next().as_deref(), args.next()) {
        (Some("--root"), Some(root)) => PathBuf::from(root),
        (None, _) => default_root()?,
        _ => return Err("usage: maka-desktop [--root <state-root>]".into()),
    };
    let host = host::Host::new().map_err(|error| error.to_string())?;
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.set_global(host);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1080.), px(760.)), cx)),
                ..Default::default()
            };
            cx.open_window(options, |window, cx| {
                let view = cx.new(|cx| workspace::Workspace::new(root, cx));
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("failed to open window");
            cx.activate(true);
        });
    Ok(())
}

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

use super::{Health, Result, TrustedError, failed};
use deno_core::{JsRuntime, RuntimeOptions, v8};
use deno_permissions::{
    Permissions, PermissionsContainer, PermissionsOptions, RuntimePermissionDescriptorParser,
};
use futures_util::future::LocalBoxFuture;
use serde_json::Value;
use std::sync::Arc;

pub(super) type Call = LocalBoxFuture<'static, Result<v8::Global<v8::Value>>>;

pub(super) fn create(health: &Arc<Health>) -> Result<JsRuntime> {
    let mut telemetry = deno_telemetry::deno_telemetry::init();
    for source in telemetry.lazy_loaded_js_files.to_mut() {
        let code = match source.specifier {
            "ext:deno_telemetry/telemetry.ts" => {
                include_str!(concat!(env!("OUT_DIR"), "/telemetry.js"))
            }
            "ext:deno_telemetry/util.ts" => include_str!(concat!(env!("OUT_DIR"), "/util.js")),
            _ => {
                return Err(TrustedError::Failed(
                    "unexpected telemetry extension source".into(),
                ));
            }
        };
        source.code = deno_core::ExtensionFileSourceCode::Computed(Arc::from(code));
    }
    let parser = Arc::new(RuntimePermissionDescriptorParser::new(
        sys_traits::impls::RealSys,
    ));
    let permissions = Permissions::from_options(
        parser.as_ref(),
        &PermissionsOptions {
            allow_net: Some(vec![]),
            prompt: false,
            ..Default::default()
        },
    )
    .map_err(|error| TrustedError::Failed(error.to_string()))?;
    let mut runtime = JsRuntime::try_new(RuntimeOptions {
        extensions: vec![
            deno_webidl::deno_webidl::init(),
            deno_web::deno_web::init(
                Arc::new(deno_web::BlobStore::default()),
                None,
                false,
                Default::default(),
            ),
            deno_fetch::deno_fetch::init(deno_fetch::Options {
                user_agent: "maka-runtime-host".into(),
                ..Default::default()
            }),
            deno_net::deno_net::init(None, None),
            telemetry,
            deno_crypto::deno_crypto::init(None),
            super::ops::maka_trusted::init(),
        ],
        create_params: Some(
            deno_core::v8::CreateParams::default().heap_limits(0, 256 * 1024 * 1024),
        ),
        ..Default::default()
    })
    .map_err(|error| TrustedError::Failed(error.to_string()))?;
    runtime
        .op_state()
        .borrow_mut()
        .put(PermissionsContainer::new(parser, permissions));

    let isolate = runtime.v8_isolate().thread_safe_handle();
    let _ = health.isolate.set(isolate);
    let watch = health.clone();
    runtime.add_near_heap_limit_callback(move |current, _| {
        watch.fail("shared V8 heap budget exceeded");
        current.saturating_add(16 * 1024 * 1024)
    });
    runtime
        .op_state()
        .borrow_mut()
        .put(super::ops::Models::default());
    for (name, source) in [
        (
            "maka:trusted/bootstrap",
            include_str!("../../trusted/bootstrap.js"),
        ),
        (
            "maka:trusted/providers",
            include_str!(concat!(env!("OUT_DIR"), "/providers.js")),
        ),
        (
            "maka:trusted/dispatch",
            include_str!("../../trusted/dispatch.js"),
        ),
    ] {
        runtime.execute_script(name, source).map_err(failed)?;
    }
    Ok(runtime)
}

pub(super) struct Functions {
    pub model: v8::Global<v8::Function>,
    pub cancel: v8::Global<v8::Function>,
}

impl Functions {
    pub fn load(runtime: &mut JsRuntime) -> Result<Self> {
        fn get(runtime: &mut JsRuntime, name: &'static str) -> Result<v8::Global<v8::Function>> {
            let value = runtime
                .execute_script("maka:trusted/bind", format!("makaTrusted.{name}"))
                .map_err(failed)?;
            deno_core::scope!(scope, runtime);
            let local = v8::Local::new(scope, value);
            let function = v8::Local::<v8::Function>::try_from(local).map_err(failed)?;
            Ok(v8::Global::new(scope, function))
        }
        Ok(Self {
            model: get(runtime, "model")?,
            cancel: get(runtime, "cancel")?,
        })
    }
}

pub(super) fn call(
    runtime: &mut JsRuntime,
    function: &v8::Global<v8::Function>,
    args: &[Value],
) -> Result<Call> {
    let values = {
        deno_core::scope!(scope, runtime);
        args.iter()
            .map(|arg| {
                deno_core::serde_v8::to_v8(scope, arg)
                    .map(|value| v8::Global::new(scope, value))
                    .map_err(failed)
            })
            .collect::<Result<Vec<_>>>()?
    };
    let promise = runtime.call_with_args(function, &values);
    Ok(Box::pin(async move { promise.await.map_err(failed) }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn bundled_telemetry_matches_the_locked_extension_sources() {
        let extension = deno_telemetry::deno_telemetry::init();
        assert_eq!(extension.lazy_loaded_js_files.len(), 2);
        for source in extension.lazy_loaded_js_files.iter() {
            let expected = match source.specifier {
                "ext:deno_telemetry/telemetry.ts" => {
                    include_str!("../../third-party/deno-telemetry/telemetry.ts")
                }
                "ext:deno_telemetry/util.ts" => {
                    include_str!("../../third-party/deno-telemetry/util.ts")
                }
                other => panic!("unbundled extension source: {other}"),
            };
            // Deno stores build-machine paths here. Never load them in production.
            assert_eq!(
                source.load().unwrap().as_str(),
                expected,
                "{}",
                source.specifier
            );
        }
    }
}

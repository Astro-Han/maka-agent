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

use super::{
    Command, Health, Reply, Result,
    engine::{self, Functions},
    failed,
    ops::{Models, Output},
};
use deno_core::{JsRuntime, v8};
use futures_util::{FutureExt, StreamExt, future::LocalBoxFuture, stream::FuturesUnordered};
use serde_json::json;
use std::{collections::HashMap, future::poll_fn, sync::Arc, task::Poll};
use tokio::sync::{OwnedSemaphorePermit, mpsc};

struct Terminal {
    _permit: OwnedSemaphorePermit,
    busy: bool,
    failed: bool,
    closing: bool,
    closed: Option<Reply>,
}
struct Model {
    _permit: OwnedSemaphorePermit,
    _input: OwnedSemaphorePermit,
    reply: Reply,
}
#[derive(Default)]
struct Objects {
    terminals: HashMap<u32, Terminal>,
    models: HashMap<u32, Model>,
    cuts: HashMap<u32, Reply>,
}
type Pending = FuturesUnordered<LocalBoxFuture<'static, (u32, Result<v8::Global<v8::Value>>)>>;

pub(super) async fn run(
    mut commands: mpsc::UnboundedReceiver<Command>,
    health: Arc<Health>,
) -> Result<()> {
    // Completion/permits stay outside the isolate's scope. On a fatal failure,
    // first drop all promise globals and the runtime (including HTTP resources),
    // only then acknowledge cleanup and release capacity.
    let mut objects = Objects::default();
    let result = async {
        let mut runtime = engine::create(&health)?;
        drive(&mut runtime, &mut objects, &mut commands, &health).await
    }
    .await;
    if let Err(error) = &result {
        health.fail(error.to_string());
    }
    let error = health.error();
    for (_, model) in objects.models {
        let _ = model.reply.send(Err(error.clone()));
    }
    for (_, reply) in objects.cuts {
        let _ = reply.send(Err(error.clone()));
    }
    for (_, terminal) in objects.terminals {
        if let Some(reply) = terminal.closed {
            let _ = reply.send(Err(error.clone()));
        }
    }
    // Queued commands were never executed; drop them only after runtime cleanup.
    commands.close();
    while let Some(command) = commands.recv().await {
        drop(command);
    }
    result
}

async fn drive(
    runtime: &mut JsRuntime,
    objects: &mut Objects,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    health: &Health,
) -> Result<()> {
    let functions = Functions::load(runtime)?;
    let mut pending = Pending::new();
    loop {
        if health.failure.lock().unwrap().is_some() {
            return Err(health.error());
        }
        // Poll completions first, then the event loop. A full output channel is
        // merely one pending op; it cannot prevent commands or other promises.
        enum Ready {
            Complete(u32, Result<v8::Global<v8::Value>>),
            Command(Option<Command>),
        }
        let ready = poll_fn(|cx| {
            if let Poll::Ready(Some((id, result))) = pending.poll_next_unpin(cx) {
                return Poll::Ready(Ok(Ready::Complete(id, result)));
            }
            if let Poll::Ready(Err(error)) = runtime.poll_event_loop(cx, Default::default()) {
                return Poll::Ready(Err(failed(error)));
            }
            // Running JS above can settle a promise without another OS event.
            if let Poll::Ready(Some((id, result))) = pending.poll_next_unpin(cx) {
                return Poll::Ready(Ok(Ready::Complete(id, result)));
            }
            commands
                .poll_recv(cx)
                .map(|command| Ok(Ready::Command(command)))
        })
        .await?;
        match ready {
            Ready::Command(None) => return Ok(()),
            Ready::Command(Some(command)) => match command {
                Command::Model {
                    id,
                    request,
                    sender,
                    cancellation,
                    lane,
                    network,
                    responses,
                    permit,
                    input,
                    reply,
                } => {
                    let endpoint = request["provider"]["baseUrl"]
                        .as_str()
                        .unwrap_or("")
                        .to_owned();
                    runtime
                        .op_state()
                        .borrow_mut()
                        .borrow_mut::<Models>()
                        .active
                        .insert(
                            id,
                            Output {
                                http: std::rc::Rc::new(super::http::Exchange::new(
                                    network.clone(),
                                    cancellation.clone(),
                                )),
                                sender,
                                responses: lane.map(|lane| {
                                    std::rc::Rc::new(super::responses::Exchange::new(
                                        lane,
                                        cancellation.clone(),
                                        network,
                                        responses,
                                    ))
                                }),
                                cancellation,
                                endpoint,
                            },
                        );
                    objects.models.insert(
                        id,
                        Model {
                            _permit: permit,
                            _input: input,
                            reply,
                        },
                    );
                    let call = engine::call(runtime, &functions.model, &[json!(id), request])?;
                    pending.push(async move { (id, call.await) }.boxed_local());
                }
                Command::Cancel(id) => {
                    sync(runtime, &functions.cancel, &[json!(id)])?;
                }
                Command::Create { id, size, permit } => {
                    objects.terminals.insert(
                        id,
                        Terminal {
                            _permit: permit,
                            busy: false,
                            failed: false,
                            closing: false,
                            closed: None,
                        },
                    );
                    if sync(runtime, &functions.create, &[json!(id), size]).is_err() {
                        objects.terminals.get_mut(&id).unwrap().failed = true;
                        sync(runtime, &functions.dispose, &[json!(id)])?;
                    }
                }
                Command::Terminal {
                    id,
                    operation,
                    reply,
                } => {
                    let Some(terminal) = objects.terminals.get_mut(&id) else {
                        let _ = reply.send(Err(failed("terminal parser closed")));
                        continue;
                    };
                    if terminal.busy || terminal.closing || terminal.failed {
                        let _ = reply.send(Err(failed("terminal parser has an incomplete cut")));
                        continue;
                    }
                    terminal.busy = true;
                    objects.cuts.insert(id, reply);
                    let (operation, argument) = operation.arguments();
                    let call = engine::call(
                        runtime,
                        &functions.terminal,
                        &[json!(id), json!(operation), argument],
                    )?;
                    pending.push(async move { (id, call.await) }.boxed_local());
                }
                Command::Dispose { id, reply } => {
                    if let Some(terminal) = objects.terminals.get_mut(&id) {
                        terminal.closing = true;
                        terminal.closed = reply;
                        if !terminal.busy {
                            dispose(runtime, &functions, objects, id)?;
                        }
                    } else if let Some(reply) = reply {
                        let _ = reply.send(Ok(String::new()));
                    }
                }
            },
            Ready::Complete(id, result) => {
                if let Some(model) = objects.models.remove(&id) {
                    runtime
                        .op_state()
                        .borrow_mut()
                        .borrow_mut::<Models>()
                        .active
                        .remove(&id);
                    let _ = model.reply.send(result.map(|_| String::new()));
                } else if let Some(reply) = objects.cuts.remove(&id) {
                    let result = result.and_then(|value| engine::string(runtime, value));
                    let failed = result.is_err() || reply.is_closed();
                    let terminal = objects.terminals.get_mut(&id).expect("live terminal cut");
                    terminal.busy = false;
                    terminal.failed |= failed;
                    if terminal.closing {
                        dispose(runtime, &functions, objects, id)?;
                    } else if failed {
                        // Keep the live handle's permit through its eventual
                        // Dispose command, even though the parser is poisoned.
                        sync(runtime, &functions.dispose, &[json!(id)])?;
                    }
                    let _ = reply.send(result);
                }
            }
        }
        // Cooperate with Tokio IO even under continuously ready commands.
        tokio::task::yield_now().await;
    }
}

fn sync(
    runtime: &mut JsRuntime,
    function: &v8::Global<v8::Function>,
    args: &[serde_json::Value],
) -> Result<()> {
    engine::call(runtime, function, args)?
        .now_or_never()
        .ok_or_else(|| failed("synchronous trusted function returned a pending promise"))??;
    Ok(())
}

fn dispose(
    runtime: &mut JsRuntime,
    functions: &Functions,
    objects: &mut Objects,
    id: u32,
) -> Result<()> {
    sync(runtime, &functions.dispose, &[json!(id)])?;
    if let Some(terminal) = objects.terminals.remove(&id)
        && let Some(reply) = terminal.closed
    {
        let _ = reply.send(Ok(String::new()));
    }
    Ok(())
}

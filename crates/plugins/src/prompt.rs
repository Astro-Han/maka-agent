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

use crate::{
    Error,
    contributions::{Captured, Contribution},
};
use maka_runtime::{
    artifact::content_digest,
    composition::{SourceKind, SourceRevision},
    event::Invocation,
};
use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

mod render;

pub type TextFuture = Pin<Box<dyn Future<Output = Result<Option<String>, Error>> + Send>>;

/// Read-only request identity, never fabricated Tool-call authority.
#[derive(Clone)]
pub struct Request {
    pub target: Target,
    pub cancellation: CancellationToken,
}

/// Pre-admission executor instructions and logical model steps are both
/// read-only prompt contexts; neither fabricates a Tool-call identity.
#[derive(Clone, serde::Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Target {
    Session { session_id: String, cwd: String },
    ModelStep { invocation: Invocation, cwd: String },
}
impl Target {
    pub fn session_id(&self) -> &str {
        match self {
            Self::Session { session_id, .. } => session_id,
            Self::ModelStep { invocation, .. } => &invocation.session_id,
        }
    }
    pub fn cwd(&self) -> &str {
        match self {
            Self::Session { cwd, .. } | Self::ModelStep { cwd, .. } => cwd,
        }
    }
}

pub trait Provider: Send + Sync {
    fn evaluate(&self, request: Request) -> TextFuture;
}

#[derive(Clone)]
pub enum Text {
    Literal(String),
    Dynamic(Arc<dyn Provider>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SectionMode {
    Append,
    Complete,
}

#[derive(Clone, Copy, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Plain,
    #[default]
    Template,
}
impl Format {
    fn render(
        self,
        text: &str,
        variables: &BTreeMap<String, Option<String>>,
    ) -> Result<String, Error> {
        match self {
            Self::Plain => Ok(text.to_owned()),
            Self::Template => render::interpolate(text, variables),
        }
    }
}

pub struct Section {
    pub format: Format,
    pub order: i32,
    pub mode: SectionMode,
    pub text: Text,
}
pub struct Variable(pub Text);
pub struct DynamicContext {
    pub format: Format,
    pub order: i32,
    pub text: Text,
}

/// Resolved once per logical step and reused unchanged by physical retries.
#[derive(Clone, Debug, Default)]
pub struct Resolved {
    pub system: Option<String>,
    pub contexts: Vec<String>,
    pub sources: Vec<SourceRevision>,
}

pub async fn resolve(
    captured: Option<&Captured>,
    base: Option<&str>,
    request: Request,
) -> Result<Resolved, Error> {
    let mut result = Resolved {
        system: base.map(str::to_owned),
        ..Resolved::default()
    };
    let Some(captured) = captured else {
        return Ok(result);
    };
    let sections = captured.typed::<Section>().entries;
    let variables = captured.typed::<Variable>().entries;
    // Temporary user context belongs to model steps, not executor system instructions.
    let contexts = if matches!(request.target, Target::Session { .. }) {
        BTreeMap::new()
    } else {
        captured.typed::<DynamicContext>().entries
    };
    if sections.is_empty() && variables.is_empty() && contexts.is_empty() {
        return Ok(result);
    }
    if sections.len() + variables.len() + contexts.len() > 128 {
        return Err(Error::Invalid("prompt contribution limit exceeded".into()));
    }
    if sections
        .values()
        .filter(|entry| entry.value.mode == SectionMode::Complete)
        .count()
        > 1
    {
        return Err(Error::Invalid("multiple complete prompt sections".into()));
    }
    // One deadline bounds the entire assembly, not 128 sequential timeouts.
    let assembly = async {
        let mut values = BTreeMap::new();
        for (name, entry) in variables {
            let text = evaluate(&entry.value.0, &entry, &request).await?;
            result
                .sources
                .push(source(&entry, SourceKind::PromptVariable, &name, &text)?);
            values.insert(name, text);
        }
        let complete = sections
            .values()
            .any(|entry| entry.value.mode == SectionMode::Complete);
        let mut rendered = Vec::new();
        // Explicit Session/behavior instructions are execution constraints, not
        // the replaceable assistant persona supplied by another Contribution.
        if let Some(base) = base {
            rendered.push((0, String::new(), base.to_owned()));
        }
        for (name, entry) in sections {
            if complete && entry.value.mode != SectionMode::Complete {
                continue;
            }
            let text = evaluate(&entry.value.text, &entry, &request)
                .await?
                .unwrap_or_default();
            let text = entry.value.format.render(&text, &values)?;
            result
                .sources
                .push(source(&entry, SourceKind::PromptSection, &name, &text)?);
            rendered.push((entry.value.order, name, text));
        }
        rendered.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        result.system = render::join(rendered.into_iter().map(|(_, _, text)| text))?;
        let mut ordered: Vec<_> = contexts.into_iter().collect();
        ordered.sort_by(|a, b| (&a.1.value.order, &a.0).cmp(&(&b.1.value.order, &b.0)));
        let mut total = 0usize;
        for (name, entry) in ordered {
            let text = evaluate(&entry.value.text, &entry, &request)
                .await?
                .unwrap_or_default();
            let text = entry.value.format.render(&text, &values)?;
            result
                .sources
                .push(source(&entry, SourceKind::PromptContext, &name, &text)?);
            total += text.len();
            if total > 64 * 1024 {
                return Err(Error::Invalid(
                    "dynamic prompt context exceeds 64 KiB".into(),
                ));
            }
            if !text.is_empty() {
                result.contexts.push(text);
            }
        }
        Ok(result)
    };
    tokio::select! {
        biased;
        _ = request.cancellation.cancelled() => Err(Error::Retired),
        result = tokio::time::timeout(Duration::from_secs(5), assembly) =>
            result.map_err(|_| Error::Invalid("prompt assembly timed out".into()))?,
    }
}

/// Final admission checks the exact registrations that produced the snapshot.
/// No provider callback runs while Host holds its admission gate.
pub fn admit(
    captured: &Captured,
    sources: &[SourceRevision],
) -> Result<Vec<crate::fiber::CallGuard>, Error> {
    fn guard<T: Send + Sync + 'static>(
        captured: &Captured,
        source: &SourceRevision,
    ) -> Result<crate::fiber::CallGuard, Error> {
        let contribution = captured
            .typed::<T>()
            .entries
            .remove(&source.name)
            .ok_or(Error::Retired)?;
        if contribution.owner.identity()?.activation != source.activation {
            return Err(Error::Retired);
        }
        contribution.admit()
    }
    sources
        .iter()
        .map(|source| match source.kind {
            SourceKind::PromptSection => guard::<Section>(captured, source),
            SourceKind::PromptVariable => guard::<Variable>(captured, source),
            _ => Err(Error::Invalid(
                "Only system-prompt sources can be admitted".into(),
            )),
        })
        .collect()
}

async fn evaluate<T>(
    text: &Text,
    entry: &Contribution<T>,
    request: &Request,
) -> Result<Option<String>, Error> {
    let _call = entry.admit()?;
    let stopping = entry.owner.stopping()?;
    let value = match text {
        Text::Literal(value) => Some(value.clone()),
        Text::Dynamic(provider) => tokio::select! {
            biased;
            _ = stopping.cancelled() => return Err(Error::Retired),
            result = provider.evaluate(request.clone()) => result?,
        },
    };
    if value.as_ref().is_some_and(|text| text.len() > 64 * 1024) {
        return Err(Error::Invalid("prompt contribution exceeds 64 KiB".into()));
    }
    Ok(value)
}

pub fn source<T>(
    entry: &Contribution<T>,
    kind: SourceKind,
    name: &str,
    value: &impl serde::Serialize,
) -> Result<SourceRevision, Error> {
    let identity = entry.owner.identity()?;
    Ok(SourceRevision {
        kind,
        name: name.into(),
        package_id: identity.package_id,
        entry_id: identity.entry_id,
        activation: identity.activation,
        revision: content_digest(
            &serde_json::to_vec(value).map_err(|error| Error::Invalid(error.to_string()))?,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        composition::Scope,
        contributions::{Catalog, Staged},
        fiber::Fiber,
    };

    struct OptionalText(Option<String>);
    impl Provider for OptionalText {
        fn evaluate(&self, _: Request) -> TextFuture {
            let text = self.0.clone();
            Box::pin(async move { Ok(text) })
        }
    }

    #[tokio::test]
    async fn missing_prompt_variable_is_not_an_empty_value_or_an_invisible_revision() {
        let owner = Fiber::new("prompt", "prompt", Scope::Profile).unwrap();
        owner.begin_loading().unwrap();
        owner.ready().unwrap();
        owner.publish().unwrap();
        let catalog = Catalog::default();
        let request = Request {
            target: Target::ModelStep {
                invocation: Invocation {
                    session_id: "session".into(),
                    turn_id: "turn".into(),
                    run_id: "run".into(),
                    invocation_id: "invocation".into(),
                },
                cwd: "/workspace".into(),
            },
            cancellation: CancellationToken::new(),
        };
        let mut revisions = Vec::new();
        for text in [None, Some(String::new())] {
            let missing = text.is_none();
            let mut staged = Staged::default();
            staged
                .insert(
                    "optional",
                    Variable(Text::Dynamic(Arc::new(OptionalText(text)))),
                )
                .unwrap();
            let registration = catalog.register(&owner.context(), staged).unwrap();
            let captured = catalog.capture(&Scope::Profile);
            let unused = resolve(Some(&captured), Some("base"), request.clone())
                .await
                .unwrap();
            assert_eq!(unused.system.as_deref(), Some("base"));
            revisions.push(unused.sources[0].revision.clone());
            let literal = resolve(
                Some(&captured),
                Some("before{{optional}}after"),
                request.clone(),
            )
            .await
            .unwrap();
            assert_eq!(literal.system.as_deref(), Some("before{{optional}}after"));
            let mut template = Staged::default();
            template
                .insert(
                    "template",
                    Section {
                        format: Format::Template,
                        order: 0,
                        mode: SectionMode::Append,
                        text: Text::Literal("before{{optional}}after".into()),
                    },
                )
                .unwrap();
            let template = catalog.register(&owner.context(), template).unwrap();
            let captured = catalog.capture(&Scope::Profile);
            let rendered = resolve(Some(&captured), None, request.clone()).await;
            if missing {
                assert!(
                    matches!(rendered, Err(Error::Invalid(message)) if message.contains("has no value"))
                );
            } else {
                assert_eq!(rendered.unwrap().system.as_deref(), Some("beforeafter"));
            }
            drop(template);
            drop(registration);
        }
        assert_ne!(revisions[0], revisions[1]);
        owner
            .shutdown(tokio::time::Instant::now() + Duration::from_secs(1))
            .await
            .unwrap();
    }
}

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

use maka_client_capability::{
    BindingMode, Endpoint, Identity, ManagedAdmissionError as Error, PrincipalKind, Registry,
};
use maka_protocol::capability::decode_replace_input;
use maka_runtime::{
    capability::{AdmissionEvidence, Manifest},
    interaction::{GrantCapability, GrantScope},
};
use serde_json::json;
use uuid::Uuid;

fn manifest(offer: &str, server: &str, names: &[&str], affinity: &str) -> Manifest {
    decode_replace_input(&json!({
        "registrationId":"r", "offers":[{
            "offerId":offer, "version":"1", "affinity":affinity,
            "hostPathAccess":"none", "label":"Capability",
            "tools":names.iter().map(|name| json!({
                "serverId":server, "name":name, "inputSchema":{"type":"object"}
            })).collect::<Vec<_>>()
        }]
    }))
    .unwrap()
}

fn registry(manifest: Manifest, kind: PrincipalKind) -> (Registry, Uuid, String) {
    let mut registry = Registry::default();
    let connection = Uuid::new_v4();
    let provider = registry
        .attach(
            connection,
            Identity {
                principal_kind: kind,
                principal_id: "owner".into(),
                client_instance_id: "desktop".into(),
                credential_bound_client_instance_id: None,
                capability_owner: None,
            },
            Endpoint::channel(32).0,
        )
        .unwrap();
    registry.replace(connection, manifest).unwrap();
    registry
        .bind_session("s", Some(connection), BindingMode::Strict)
        .unwrap();
    (registry, connection, provider)
}

#[test]
fn browser_scopes_are_canonical_and_keep_frozen_provider_and_contract() {
    let names = [
        "browser_navigate",
        "browser_snapshot",
        "browser_click",
        "browser_type",
        "browser_wait",
        "browser_extract",
    ];
    let original = manifest("desktop_browser", "desktop_browser", &names, "session");
    let (mut registry, connection, provider) =
        registry(original.clone(), PrincipalKind::LocalOwner);
    let snapshot = registry.snapshot("s").unwrap();
    let offer = &snapshot.offers()[0];
    let registration = offer.resolve(&registry).unwrap();
    for (url, origin) in [
        (
            "HTTPS://user:pass@EXAMPLE.com:443/a?q=1#fragment",
            "https://example.com",
        ),
        ("http://example.com:8080/a", "http://example.com:8080"),
        ("https://bücher.example/a", "https://xn--bcher-kva.example"),
        ("http://[::1]:80/a", "http://[::1]"),
    ] {
        for name in names {
            let target = offer
                .managed_target(
                    &registration,
                    "desktop_browser",
                    name,
                    &AdmissionEvidence::BrowserUrl { url: url.into() },
                )
                .unwrap()
                .unwrap();
            assert_eq!(target.provider_id, provider);
            assert_eq!(target.contract_id, offer.contract_id().as_str());
            assert_eq!(target.server_id, "desktop_browser");
            assert_eq!(target.tool_name, name);
            assert_eq!(target.capability, GrantCapability::Browser);
            assert_eq!(
                target.scope,
                GrantScope::BrowserOrigin {
                    origin: origin.into()
                }
            );
        }
    }
    for url in [
        "/relative",
        "https://",
        "file:///tmp/a",
        "data:text/plain,a",
        "ftp://example.com",
    ] {
        assert_eq!(
            offer.managed_target(
                &registration,
                "desktop_browser",
                names[0],
                &AdmissionEvidence::BrowserUrl { url: url.into() }
            ),
            Err(Error::InvalidBrowserUrl)
        );
    }
    assert_eq!(
        offer.managed_target(
            &registration,
            "desktop_browser",
            names[0],
            &AdmissionEvidence::None
        ),
        Err(Error::InvalidEvidence)
    );
    assert_eq!(
        offer.managed_target(
            &registration,
            "desktop_browser",
            "unpublished",
            &AdmissionEvidence::None
        ),
        Err(Error::UnknownTool)
    );
    let evidence = AdmissionEvidence::BrowserUrl {
        url: "https://example.com/path".into(),
    };
    let old = offer
        .managed_target(&registration, "desktop_browser", names[0], &evidence)
        .unwrap();
    let mut changed = original;
    changed.registration_id = "r2".into();
    changed.offers[0].tools[0]
        .input_schema
        .insert("required".into(), json!(["url"]));
    registry.replace(connection, changed).unwrap();
    registry
        .bind_session("new", Some(connection), BindingMode::Strict)
        .unwrap();
    let new = registry.snapshot("new").unwrap();
    let target = new.offers()[0]
        .managed_target(
            &new.offers()[0].resolve(&registry).unwrap(),
            "desktop_browser",
            names[0],
            &evidence,
        )
        .unwrap();
    assert_ne!(
        old, target,
        "schema changes must not inherit the old grant identity"
    );
    assert_eq!(
        old,
        offer
            .managed_target(&registration, "desktop_browser", names[0], &evidence)
            .unwrap()
    );
}

#[test]
fn managed_policy_is_exact_and_requires_trusted_resolved_publication() {
    let evidence = AdmissionEvidence::BrowserUrl {
        url: "https://example.com".into(),
    };
    for name in ["MakaClientSettingsGet", "MakaClientSettingsUpdate"] {
        let (registry, _, _) = registry(
            manifest("desktop_settings", "desktop_settings", &[name], "session"),
            PrincipalKind::LocalOwner,
        );
        let snapshot = registry.snapshot("s").unwrap();
        let offer = &snapshot.offers()[0];
        let registration = offer.resolve(&registry).unwrap();
        assert_eq!(
            offer.managed_target(
                &registration,
                "desktop_settings",
                name,
                &AdmissionEvidence::None
            ),
            Ok(None)
        );
        assert_eq!(
            offer.managed_target(&registration, "desktop_settings", name, &evidence),
            Err(Error::InvalidEvidence)
        );
    }
    for id in ["desktop_mcp", "desktop_mcp_server", "desktop_mcp_server_2"] {
        let (registry, _, _) = registry(
            manifest(id, "real_server", &["read", "write"], "session"),
            PrincipalKind::CapabilityProvider,
        );
        let snapshot = registry.snapshot("s").unwrap();
        let offer = &snapshot.offers()[0];
        let registration = offer.resolve(&registry).unwrap();
        for name in ["read", "write"] {
            let target = offer
                .managed_target(&registration, "real_server", name, &AdmissionEvidence::None)
                .unwrap()
                .unwrap();
            assert_eq!(target.capability, GrantCapability::DesktopMcp);
            assert_eq!(
                target.scope,
                GrantScope::McpTool {
                    server_id: "real_server".into(),
                    tool_name: name.into()
                }
            );
            assert_eq!(
                offer.managed_target(&registration, "real_server", name, &evidence),
                Err(Error::InvalidEvidence)
            );
        }
    }
    for (id, server, name) in [
        ("desktop_browser", "desktop_browser", "browser_unknown"),
        ("desktop_browser", "impostor", "browser_navigate"),
        ("impostor", "desktop_browser", "browser_navigate"),
        ("desktop_settings", "desktop_settings", "unknown"),
        ("desktop_settings", "impostor", "MakaClientSettingsGet"),
        ("desktop_mcpish", "real_server", "read"),
        ("computer_use", "computer_use", "click"),
    ] {
        let (registry, _, _) = registry(
            manifest(id, server, &[name], "session"),
            PrincipalKind::LocalOwner,
        );
        assert_eq!(
            registry.snapshot("s").unwrap().offers()[0].managed_target(
                &registry.snapshot("s").unwrap().offers()[0]
                    .resolve(&registry)
                    .unwrap(),
                server,
                name,
                &AdmissionEvidence::None
            ),
            Err(Error::UnknownPolicy)
        );
    }
    for (kind, affinity) in [
        (PrincipalKind::RemoteOwner, "session"),
        (PrincipalKind::RemoteOwner, "call"),
    ] {
        let (registry, _, _) = registry(
            manifest(
                "desktop_settings",
                "desktop_settings",
                &["MakaClientSettingsGet"],
                affinity,
            ),
            kind,
        );
        assert_eq!(
            registry.snapshot("s").unwrap().offers()[0].managed_target(
                &registry.snapshot("s").unwrap().offers()[0]
                    .resolve(&registry)
                    .unwrap(),
                "desktop_settings",
                "MakaClientSettingsGet",
                &AdmissionEvidence::None
            ),
            Err(Error::UntrustedProvider)
        );
    }
}

#[test]
fn call_affinity_grants_follow_the_resolved_provider_without_rebinding() {
    let publication = manifest("desktop_mcp", "real_server", &["read"], "call");
    let (mut registry, connection, _) = registry(publication.clone(), PrincipalKind::LocalOwner);
    // No initiating selector: subsequent invocations may select another provider.
    let snapshot = registry.snapshot("unbound").unwrap();
    let offer = &snapshot.offers()[0];
    let first = offer.resolve(&registry).unwrap();
    let before = offer
        .managed_target(&first, "real_server", "read", &AdmissionEvidence::None)
        .unwrap()
        .unwrap();
    registry.detach(connection);
    let next = Uuid::new_v4();
    let provider = registry
        .attach(
            next,
            maka_client_capability::Identity {
                principal_kind: PrincipalKind::LocalOwner,
                principal_id: "owner".into(),
                client_instance_id: "other_desktop".into(),
                credential_bound_client_instance_id: None,
                capability_owner: None,
            },
            Endpoint::channel(32).0,
        )
        .unwrap();
    registry.replace(next, publication).unwrap();
    let second = offer.resolve(&registry).unwrap();
    let after = offer
        .managed_target(&second, "real_server", "read", &AdmissionEvidence::None)
        .unwrap()
        .unwrap();
    assert_eq!(after.provider_id, provider);
    assert_ne!(before.provider_id, after.provider_id);
    assert_eq!(before.contract_id, after.contract_id);
    assert_eq!(
        offer
            .managed_target(&first, "real_server", "read", &AdmissionEvidence::None)
            .unwrap()
            .unwrap(),
        before
    );
    let mut changed = manifest("desktop_mcp", "real_server", &["read"], "call");
    changed.registration_id = "changed".into();
    changed.offers[0].version = "2".into();
    registry.replace(next, changed).unwrap();
    assert_eq!(
        offer.managed_target(
            &registry.current(&provider).unwrap(),
            "real_server",
            "read",
            &AdmissionEvidence::None
        ),
        Err(Error::WrongRegistration)
    );
}

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
    BindingError, BindingMode, Endpoint, Identity, PrincipalKind, Registry,
};
use maka_protocol::capability::decode_replace_input;
use maka_runtime::capability::{HostFrame, Manifest};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

fn identity(client: &str) -> Identity {
    Identity {
        principal_kind: PrincipalKind::LocalOwner,
        principal_id: "owner".into(),
        client_instance_id: client.into(),
        credential_bound_client_instance_id: None,
        capability_owner: None,
    }
}
fn manifest(id: &str, affinities: &[&str]) -> Manifest {
    decode_replace_input(&json!({"registrationId":id,"offers":affinities.iter().map(|affinity|json!({
        "offerId":affinity,"version":"1","affinity":affinity,"hostPathAccess":"none","label":affinity,
        "tools":[{"serverId":affinity,"name":"effect","inputSchema":{"type":"object"}}]
    })).collect::<Vec<_>>()})).unwrap()
}
fn attach(
    registry: &mut Registry,
    identity: Identity,
) -> (Uuid, String, tokio::sync::mpsc::Receiver<HostFrame>) {
    let id = Uuid::new_v4();
    let (endpoint, out) = Endpoint::channel(32);
    let provider = registry.attach(id, identity, endpoint).unwrap();
    (id, provider, out)
}

#[tokio::test]
async fn scoped_publications_isolate_selection_restore_and_retire_all_pinned_generations() {
    let mut registry = Registry::default();
    let (one, _, _out) = attach(&mut registry, identity("desktop"));
    let scoped = |id: &str, session: &str| {
        let mut m = manifest(id, &["session"]);
        m.session_id = Some(session.into());
        m
    };
    registry.replace(one, scoped("a1", "a")).unwrap();
    registry.replace(one, scoped("b1", "b")).unwrap();
    assert!(
        registry
            .replace(one, manifest("collision", &["session"]))
            .is_err()
    );
    assert!(
        registry
            .preview_bindings(None, Some(one), BindingMode::Strict)
            .unwrap()
            .offers()
            .is_empty()
    );
    let (prepared, a1) = registry
        .prepare_bindings("a", Some(one), BindingMode::Strict)
        .unwrap();
    let composition = prepared.composition();
    assert!(registry.commit_bindings(prepared).unwrap());
    registry
        .bind_session("b", Some(one), BindingMode::Strict)
        .unwrap();
    let b = registry.snapshot("b").unwrap();
    let broker = maka_client_capability::broker::Broker::default();
    let call = |session: &str| maka_client_capability::broker::ToolCall {
        offer_id: "session".into(),
        server_id: "session".into(),
        tool_name: "effect".into(),
        arguments: Default::default(),
        source: maka_runtime::capability::CallSource::Agent {
            session_id: session.into(),
            turn_id: "turn".into(),
        },
        tool_call_id: "tool".into(),
        cwd: "/not-forwarded".into(),
    };
    assert!(
        broker
            .prepare_tool(
                b.offers()[0].resolve(&registry).unwrap(),
                call("a"),
                std::time::Duration::from_secs(1),
                Default::default()
            )
            .is_err()
    );
    let pending = broker
        .prepare_tool(
            b.offers()[0].resolve(&registry).unwrap(),
            call("b"),
            std::time::Duration::from_secs(10),
            Default::default(),
        )
        .unwrap();
    assert_eq!(a1.offers().len(), 1);
    assert_eq!(
        a1.offers()[0]
            .resolve(&registry)
            .unwrap()
            .manifest()
            .registration_id,
        "a1"
    );
    assert_eq!(
        b.offers()[0]
            .resolve(&registry)
            .unwrap()
            .manifest()
            .registration_id,
        "b1"
    );
    assert!(matches!(
        registry.restore_bindings("b", &composition),
        Err(BindingError::InvalidComposition)
    ));
    registry.replace(one, scoped("a2", "a")).unwrap();
    let a2 = registry.snapshot("a").unwrap();
    assert!(
        a1.offers()[0].resolve(&registry).is_ok(),
        "same connection replacement keeps frozen calls"
    );
    let (two, _, _out2) = attach(&mut registry, identity("desktop"));
    registry.unregister(one, "a2").unwrap();
    assert!(a2.offers()[0].resolve(&registry).is_ok());
    registry.replace(two, scoped("a3", "a")).unwrap();
    assert!(a1.offers()[0].resolve(&registry).is_err());
    assert!(a2.offers()[0].resolve(&registry).is_err());
    assert!(
        b.offers()[0].resolve(&registry).is_ok(),
        "another Session keeps its original connection"
    );
    let (prepared, _) = registry.restore_bindings("a", &composition).unwrap();
    assert!(registry.commit_restored_bindings(prepared).unwrap());
    let a3 = registry.snapshot("a").unwrap();
    registry
        .replace(one, manifest("global", &["turn"]))
        .unwrap();
    registry
        .replace(two, manifest("global-reconnected", &["turn"]))
        .unwrap();
    assert!(
        b.offers()[0].resolve(&registry).is_ok(),
        "global supersession must not cancel scoped calls"
    );
    registry.replace(one, scoped("b2", "b")).unwrap();
    registry.detach(two);
    assert!(a3.offers()[0].resolve(&registry).is_err());
    assert!(
        registry
            .bind_session("a", None, BindingMode::Strict)
            .is_err()
    );
    let mut empty = scoped("a-empty", "a");
    empty.offers.clear();
    registry.replace(one, empty).unwrap();
    registry
        .bind_session("a", None, BindingMode::Strict)
        .unwrap();
    assert!(
        registry.snapshot("a").unwrap().offers().is_empty(),
        "empty scoped publication clears lost contracts"
    );
    let (prepared, _) = registry
        .prepare_bindings("b", Some(one), BindingMode::Strict)
        .unwrap();
    registry.release_session("b");
    assert!(!registry.commit_bindings(prepared).unwrap());
    assert!(
        b.offers()[0].resolve(&registry).is_err(),
        "retirement reaches pinned prior generations"
    );
    assert!(matches!(
        pending.accepted().await,
        Err(maka_client_capability::broker::CallError::CapabilityLost)
    ));
    broker.shutdown().await;
    let (prepared, _) = registry
        .prepare_bindings("new", None, BindingMode::Strict)
        .unwrap();
    registry.release_session("new");
    assert!(
        !registry.commit_bindings(prepared).unwrap(),
        "retirement also invalidates an empty preview"
    );
}

#[test]
fn restart_restores_identity_ceiling_without_discovery_or_call_pinning() {
    for initiating in [false, true] {
        let mut old = Registry::default();
        let owner = identity("desktop");
        let (connection, _, _out) = attach(&mut old, owner.clone());
        old.replace(connection, manifest("old", &["session", "turn", "call"]))
            .unwrap();
        let (prepared, _) = old
            .prepare_bindings("s", initiating.then_some(connection), BindingMode::Strict)
            .unwrap();
        let composition =
            serde_json::from_value(serde_json::to_value(prepared.composition()).unwrap()).unwrap();
        drop(old);

        let mut registry = Registry::default();
        assert!(matches!(
            registry.restore_bindings("s", &composition),
            Err(BindingError::Lost)
        ));
        let (other, _, _other_out) = attach(&mut registry, identity("another-desktop"));
        registry
            .replace(other, manifest("other", &["session", "turn", "call"]))
            .unwrap();
        assert!(matches!(
            registry.restore_bindings("s", &composition),
            Err(BindingError::Lost)
        ));
        let (connection, _, _out) = attach(&mut registry, owner);
        registry
            .replace(
                connection,
                manifest("current", &["session", "turn", "call"]),
            )
            .unwrap();
        let (prepared, _) = registry.restore_bindings("s", &composition).unwrap();
        registry
            .replace(
                connection,
                manifest("replaced", &["session", "turn", "call"]),
            )
            .unwrap();
        assert!(!registry.commit_restored_bindings(prepared).unwrap());
        let (prepared, restored) = registry.restore_bindings("s", &composition).unwrap();
        assert!(registry.commit_restored_bindings(prepared).unwrap());
        assert_eq!(restored.offers().len(), 3);
        let call = restored
            .offers()
            .iter()
            .find(|o| o.offer().offer_id == "call")
            .unwrap();
        if initiating {
            assert_eq!(call.resolve(&registry).unwrap().connection_id(), connection);
        } else {
            assert!(matches!(
                call.resolve(&registry),
                Err(BindingError::Ambiguous)
            ));
        }
        registry.detach(connection);
        if initiating {
            assert!(matches!(call.resolve(&registry), Err(BindingError::Lost)));
        } else {
            assert_eq!(call.resolve(&registry).unwrap().connection_id(), other);
        }
        assert!(
            restored
                .offers()
                .iter()
                .filter(|o| o.offer().affinity != maka_runtime::capability::Affinity::Call)
                .all(|o| matches!(o.resolve(&registry), Err(BindingError::Lost)))
        );
    }

    // A lost owner is absent from this Run's tools but remains an authentication
    // boundary, even if it reconnects before or after restoration.
    let mut old = Registry::default();
    let owner = identity("lost");
    let (connection, _, _out) = attach(&mut old, owner.clone());
    old.replace(connection, manifest("old", &["session"]))
        .unwrap();
    old.bind_session("s", Some(connection), BindingMode::Strict)
        .unwrap();
    old.detach(connection);
    let (prepared, _) = old
        .prepare_bindings("s", None, BindingMode::Degrade)
        .unwrap();
    let proof = prepared.composition();
    assert_eq!(proof.session_bindings.len(), 1);
    assert!(proof.offers.is_empty());
    drop(old);
    let mut registry = Registry::default();
    let (prepared, restored) = registry.restore_bindings("s", &proof).unwrap();
    assert!(registry.commit_restored_bindings(prepared).unwrap());
    let mut changed = owner.clone();
    changed.credential_bound_client_instance_id = Some("lost".into());
    let (endpoint, _out) = Endpoint::channel(32);
    assert!(registry.attach(Uuid::new_v4(), changed, endpoint).is_err());
    let (connection, _, _out) = attach(&mut registry, owner);
    registry
        .replace(connection, manifest("new", &["session"]))
        .unwrap();
    assert!(restored.offers().is_empty());
    assert_eq!(registry.snapshot("s").unwrap().offers().len(), 1);
    let (prepared, restored) = registry.restore_bindings("s", &proof).unwrap();
    assert!(registry.commit_restored_bindings(prepared).unwrap());
    assert!(restored.offers().is_empty());
}

#[test]
fn session_loss_restore_retirement_and_run_pins_are_distinct() {
    let mut registry = Registry::default();
    let owner = identity("desktop");
    let (one, provider, mut out) = attach(&mut registry, owner.clone());
    registry
        .replace(one, manifest("a", &["session", "turn", "call"]))
        .unwrap();
    let preview = registry
        .preview_bindings(None, Some(one), BindingMode::Strict)
        .unwrap();
    assert_eq!(preview.offers().len(), 3);
    assert_eq!(
        registry.snapshot("s").unwrap().offers().len(),
        1,
        "preview does not publish Session/Turn bindings"
    );
    drop(preview);
    let (prepared, candidate) = registry
        .prepare_bindings("s", Some(one), BindingMode::Strict)
        .unwrap();
    assert_eq!(registry.snapshot("s").unwrap().offers().len(), 1);
    assert_eq!(candidate.offers().len(), 3);
    drop(candidate);
    assert!(registry.commit_bindings(prepared).unwrap());
    let old = registry.snapshot("s").unwrap();
    assert_eq!(old.offers().len(), 3);
    assert_eq!(
        old.offers().iter().filter(|o| o.trusted()).count(),
        2,
        "call affinity is never trusted from its representative"
    );
    let weak = Arc::downgrade(&registry.current(&provider).unwrap());
    let (prepared, candidate) = registry
        .prepare_bindings("s", Some(one), BindingMode::Strict)
        .unwrap();
    drop(candidate);
    registry
        .replace(one, manifest("b", &["session", "turn", "call"]))
        .unwrap();
    assert!(
        !registry.commit_bindings(prepared).unwrap(),
        "publication replacement invalidates prepared handlers"
    );
    for offer in old.offers() {
        let registration = offer.resolve(&registry).unwrap();
        let expected = if offer.offer().offer_id == "call" {
            "b"
        } else {
            "a"
        };
        assert_eq!(registration.manifest().registration_id, expected);
    }
    assert!(
        out.try_recv().is_err(),
        "run snapshot must keep exact retired registration resident"
    );
    drop(old);
    assert!(weak.upgrade().is_none());
    assert!(
        matches!(out.try_recv().unwrap(),HostFrame::RegistrationRelease{registration_id} if registration_id=="a")
    );
    let before_disconnect = registry.snapshot("s").unwrap();
    registry.detach(one);
    for offer in before_disconnect.offers() {
        assert!(matches!(offer.resolve(&registry), Err(BindingError::Lost)));
    }
    drop(before_disconnect);
    assert!(registry.snapshot("s").unwrap().offers().is_empty());
    assert_eq!(
        registry.bind_session("s", None, BindingMode::Strict),
        Err(BindingError::Lost)
    );
    assert!(matches!(
        registry.preview_bindings(Some("s"), None, BindingMode::Strict),
        Err(BindingError::Lost)
    ));
    assert!(
        registry
            .preview_bindings(Some("s"), None, BindingMode::Degrade)
            .unwrap()
            .offers()
            .is_empty()
    );
    assert_eq!(
        registry.bind_session("s", None, BindingMode::Strict),
        Err(BindingError::Lost),
        "degraded preview cannot erase lost binding authority"
    );
    registry
        .bind_session("s", None, BindingMode::Degrade)
        .unwrap();
    // Lost bindings alone retain authentication authority after every old
    // publication and socket has gone.
    let mut forged = owner.clone();
    forged.credential_bound_client_instance_id = Some("changed".into());
    assert!(
        registry
            .attach(Uuid::new_v4(), forged, Endpoint::channel(8).0)
            .is_err()
    );
    let (two, _, mut out_two) = attach(&mut registry, owner);
    registry
        .replace(two, manifest("c", &["session", "turn", "call"]))
        .unwrap();
    let restored = registry.snapshot("s").unwrap();
    assert_eq!(
        restored.offers().len(),
        2,
        "reconnect restores session, not the lost turn binding"
    );
    drop(restored);
    registry
        .bind_session("s", Some(two), BindingMode::Strict)
        .unwrap();
    let pinned = registry.snapshot("s").unwrap();
    registry.unregister(two, "c").unwrap();
    assert!(
        registry.snapshot("s").unwrap().offers().is_empty(),
        "explicit unregister removes bindings rather than making them lost"
    );
    for offer in pinned.offers() {
        if offer.offer().offer_id != "call" {
            assert!(offer.resolve(&registry).is_ok());
        }
    }
    assert!(out_two.try_recv().is_err());
    drop(pinned);
    assert!(
        matches!(out_two.try_recv().unwrap(),HostFrame::RegistrationRelease{registration_id} if registration_id=="c")
    );
    registry.detach(two);
    let (three, _, _) = attach(&mut registry, identity("replacement"));
    registry
        .replace(three, manifest("d", &["session", "turn", "call"]))
        .unwrap();
    registry
        .bind_session("s", None, BindingMode::Strict)
        .unwrap();
    assert_eq!(registry.snapshot("s").unwrap().offers().len(), 3);
    let (prepared, candidate) = registry
        .prepare_bindings("s", None, BindingMode::Strict)
        .unwrap();
    drop(candidate);
    registry.release_session("s");
    assert!(
        !registry.commit_bindings(prepared).unwrap(),
        "preparation cannot restore bindings released by another admission"
    );
    assert_eq!(
        registry.snapshot("s").unwrap().offers().len(),
        1,
        "only globally discoverable call affinity remains"
    );
}

#[test]
fn required_tools_commit_only_one_session_owner_and_failed_selection_leaves_no_binding() {
    let offers = |id: &str, affinity: &str, names: &[&str]| {
        decode_replace_input(&json!({"registrationId": id, "offers": names.iter().map(|name| json!({
            "offerId": name, "version": "1", "affinity": affinity, "hostPathAccess": "none",
            "label": name, "tools": [{"serverId": "desktop", "name": name, "inputSchema": {"type": "object"}}]
        })).collect::<Vec<_>>()})).unwrap()
    };
    let required = ["mcp__desktop__control", "mcp__desktop__tasks"];
    let optional = ["mcp__desktop__browser"];
    let mut registry = Registry::default();
    let (first, _, _first_output) = attach(&mut registry, identity("first"));
    registry
        .replace(first, offers("partial", "session", &["control"]))
        .unwrap();
    assert!(matches!(
        registry.bind_required_tools("fresh", first, &required, &[]),
        Err(BindingError::RequiredProvider)
    ));
    assert!(registry.snapshot("fresh").unwrap().offers().is_empty());
    registry
        .bind_session("mixed", Some(first), BindingMode::Strict)
        .unwrap();
    let (second, provider, _second_output) = attach(&mut registry, identity("second"));
    registry
        .replace(second, offers("other", "session", &["tasks"]))
        .unwrap();
    assert!(matches!(
        registry.bind_required_tools("mixed", second, &required, &[]),
        Err(BindingError::RequiredProvider)
    ));
    assert_eq!(registry.snapshot("mixed").unwrap().offers().len(), 1);
    registry.detach(first);
    for affinity in ["turn", "call"] {
        registry
            .replace(second, offers(affinity, affinity, &["control", "tasks"]))
            .unwrap();
        assert!(matches!(
            registry.bind_required_tools("fresh", second, &required, &[]),
            Err(BindingError::RequiredProvider)
        ));
    }
    registry
        .replace(second, offers("complete", "session", &["control", "tasks"]))
        .unwrap();
    let (prepared, preview) = registry
        .prepare_required_tools("fresh", Some(second), &required, &optional)
        .unwrap();
    assert_eq!(preview.offers().len(), 2);
    assert!(registry.snapshot("fresh").unwrap().offers().is_empty());
    registry
        .replace(second, offers("renewed", "session", &["control", "tasks"]))
        .unwrap();
    assert!(!registry.commit_bindings(prepared).unwrap());
    assert!(registry.snapshot("fresh").unwrap().offers().is_empty());
    let (snapshot, _) = registry
        .bind_required_tools("fresh", second, &required, &optional)
        .unwrap();
    assert_eq!(snapshot.offers().len(), 2);
    for offer in snapshot.offers() {
        assert_eq!(offer.resolve(&registry).unwrap().provider_id(), provider);
    }
    registry
        .replace(
            second,
            offers(
                "with-browser",
                "session",
                &["control", "tasks", "browser", "unrelated"],
            ),
        )
        .unwrap();
    let (snapshot, _) = registry
        .bind_required_tools("fresh", second, &required, &optional)
        .unwrap();
    assert_eq!(
        snapshot.offers().len(),
        3,
        "optional tools are included, unrelated offers remain outside the profile"
    );
    assert!(
        matches!(
            registry.bind_required_tools("mixed", second, &required, &[]),
            Err(BindingError::Lost)
        ),
        "A real prior binding remains authoritative; only failed selections leave nothing behind"
    );
}

#[test]
fn dynamic_calls_do_not_pin_and_remote_selection_never_inherits_unrelated_authority() {
    let mut registry = Registry::default();
    let (one, provider, mut out) = attach(&mut registry, identity("one"));
    registry.replace(one, manifest("a", &["call"])).unwrap();
    let dynamic = registry.snapshot("unbound").unwrap();
    let weak = Arc::downgrade(&registry.current(&provider).unwrap());
    registry.replace(one, manifest("b", &["call"])).unwrap();
    assert!(
        weak.upgrade().is_none(),
        "call-affinity snapshot must not retain its representative publication"
    );
    assert!(
        matches!(out.try_recv().unwrap(),HostFrame::RegistrationRelease{registration_id} if registration_id=="a")
    );
    assert_eq!(
        dynamic.offers()[0]
            .resolve(&registry)
            .unwrap()
            .manifest()
            .registration_id,
        "b"
    );
    let (two, _, _) = attach(&mut registry, identity("two"));
    registry.replace(two, manifest("c", &["call"])).unwrap();
    let preview = registry
        .preview_bindings(Some("unbound"), Some(two), BindingMode::Strict)
        .unwrap();
    assert_eq!(
        preview.offers()[0]
            .resolve(&registry)
            .unwrap()
            .manifest()
            .registration_id,
        "c"
    );
    assert!(
        matches!(
            registry.snapshot("unbound").unwrap().offers()[0].resolve(&registry),
            Err(BindingError::Ambiguous)
        ),
        "preview must not replace the initiating selector"
    );
    drop(preview);
    assert!(matches!(
        dynamic.offers()[0].resolve(&registry),
        Err(BindingError::Ambiguous)
    ));
    registry
        .bind_session("selected", Some(one), BindingMode::Strict)
        .unwrap();
    let selected = registry.snapshot("selected").unwrap();
    assert_eq!(
        selected.offers()[0]
            .resolve(&registry)
            .unwrap()
            .provider_id(),
        provider
    );
    registry.detach(one);
    assert!(
        matches!(
            selected.offers()[0].resolve(&registry),
            Err(BindingError::Lost)
        ),
        "never fall back to unrelated provider"
    );
    let mut remote = identity("remote");
    remote.principal_kind = PrincipalKind::RemoteOwner;
    remote.credential_bound_client_instance_id = Some("remote".into());
    let (remote_connection, _, _) = attach(&mut registry, remote.clone());
    registry
        .bind_session("remote", Some(remote_connection), BindingMode::Strict)
        .unwrap();
    assert!(
        registry.snapshot("remote").unwrap().offers().is_empty(),
        "sole unrelated candidate remains hidden"
    );
    let mut delegated = identity("delegated");
    delegated.principal_kind = PrincipalKind::CapabilityProvider;
    delegated.capability_owner = Some(maka_runtime::access::CapabilityOwnerIdentity {
        principal_id: remote.principal_id.clone(),
        client_instance_id: remote.client_instance_id.clone(),
    });
    let (delegated_connection, delegated_provider, _) = attach(&mut registry, delegated.clone());
    registry
        .replace(delegated_connection, manifest("d", &["session"]))
        .unwrap();
    registry
        .bind_session("remote", Some(remote_connection), BindingMode::Strict)
        .unwrap();
    let snapshot = registry.snapshot("remote").unwrap();
    assert_eq!(snapshot.offers().len(), 1);
    assert_eq!(
        snapshot.offers()[0]
            .resolve(&registry)
            .unwrap()
            .provider_id(),
        delegated_provider
    );
    delegated.client_instance_id = "another-delegated".into();
    let (other, _, _) = attach(&mut registry, delegated);
    registry
        .replace(other, manifest("e", &["session"]))
        .unwrap();
    assert_eq!(
        registry.bind_session("remote", Some(remote_connection), BindingMode::Strict),
        Err(BindingError::Ambiguous)
    );
    assert_eq!(
        registry.snapshot("remote").unwrap().offers()[0]
            .resolve(&registry)
            .unwrap()
            .provider_id(),
        delegated_provider,
        "failed selection must leave prior binding unchanged"
    );
    registry.begin_drain();
    assert!(matches!(
        snapshot.offers()[0].resolve(&registry),
        Err(BindingError::Lost)
    ));
    assert!(matches!(
        registry.snapshot("remote"),
        Err(BindingError::Draining)
    ));
}

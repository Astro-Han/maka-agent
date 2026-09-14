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
    registry
        .bind_session("s", Some(one), BindingMode::Strict)
        .unwrap();
    let old = registry.snapshot("s").unwrap();
    assert_eq!(old.offers().len(), 3);
    assert_eq!(
        old.offers().iter().filter(|o| o.trusted()).count(),
        2,
        "call affinity is never trusted from its representative"
    );
    let weak = Arc::downgrade(&registry.current(&provider).unwrap());
    registry
        .replace(one, manifest("b", &["session", "turn", "call"]))
        .unwrap();
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
    registry.release_session("s");
    assert_eq!(
        registry.snapshot("s").unwrap().offers().len(),
        1,
        "only globally discoverable call affinity remains"
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

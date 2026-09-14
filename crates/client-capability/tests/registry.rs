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

use maka_client_capability::{Endpoint, Identity, PrincipalKind, Registry};
use maka_protocol::capability::decode_replace_input;
use maka_runtime::{access::CapabilityOwnerIdentity, capability::HostFrame};
use serde_json::json;
use uuid::Uuid;

fn identity() -> Identity {
    Identity {
        principal_kind: PrincipalKind::LocalOwner,
        principal_id: "local-owner".into(),
        client_instance_id: "desktop".into(),
        credential_bound_client_instance_id: None,
        capability_owner: None,
    }
}
fn manifest(id: &str) -> maka_runtime::capability::Manifest {
    decode_replace_input(&json!({"registrationId":id, "offers":[{
        "offerId":"browser", "version":"1", "affinity":"session",
        "hostPathAccess":"none", "label":"Browser", "tools":[{
            "serverId":"desktop_browser", "name":"browser_snapshot", "inputSchema":{"type":"object"}
        }]
    }]}))
    .unwrap()
}
fn release(frame: HostFrame, id: &str) {
    assert!(
        matches!(frame, HostFrame::RegistrationRelease { registration_id } if registration_id == id)
    );
}

#[test]
fn replacing_and_reconnecting_preserves_exact_pins_and_original_release_owner() {
    let mut registry = Registry::default();
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let (one, mut out_one) = Endpoint::channel(8);
    let old_invocations = one.invocations();
    let old_closed = one.closed();
    let provider = registry.attach(first, identity(), one).unwrap();
    assert_eq!(registry.replace(first, manifest("a")).unwrap().revision, 1);
    let snapshot = registry.current(&provider).unwrap();
    let invocation = snapshot.clone();
    assert_eq!(registry.replace(first, manifest("b")).unwrap().revision, 2);
    assert!(!old_invocations.is_cancelled());
    assert!(out_one.try_recv().is_err());
    assert!(registry.replace(first, manifest("a")).is_err());
    drop(snapshot);
    assert!(out_one.try_recv().is_err());
    drop(invocation);
    release(out_one.try_recv().unwrap(), "a");
    // IDs cease being resident only after their release has been queued.
    registry.replace(first, manifest("a")).unwrap();
    release(out_one.try_recv().unwrap(), "b");
    let pinned = registry.current(&provider).unwrap();
    let (two, mut out_two) = Endpoint::channel(8);
    assert_eq!(registry.attach(second, identity(), two).unwrap(), provider);
    assert!(!old_invocations.is_cancelled(), "attach is not activation");
    assert_eq!(registry.current(&provider).unwrap().connection_id(), first);
    assert!(registry.replace(second, manifest("a")).is_err());
    assert!(
        !old_invocations.is_cancelled(),
        "failed publication cannot supersede"
    );
    for (left, right) in [("foo.bar", "foo_bar"), ("Ｆｏｏ", "Foo"), ("⛵", "unnamed")] {
        let mut collision = manifest("collision");
        collision.offers[0].tools[0].name = left.into();
        let mut other = collision.offers[0].tools[0].clone();
        other.name = right.into();
        collision.offers[0].tools.push(other);
        assert!(registry.replace(second, collision).is_err());
        assert!(!old_invocations.is_cancelled());
        assert_eq!(
            registry
                .current(&provider)
                .unwrap()
                .manifest()
                .registration_id,
            "a"
        );
    }
    registry.replace(second, manifest("c")).unwrap();
    assert!(old_invocations.is_cancelled());
    assert!(
        !old_closed.is_cancelled(),
        "supersession leaves control channel alive"
    );
    assert!(registry.replace(first, manifest("d")).is_err());
    assert!(registry.unregister(first, "c").is_err());
    assert!(out_one.try_recv().is_err());
    drop(pinned);
    release(out_one.try_recv().unwrap(), "a");
    assert!(
        out_two.try_recv().is_err(),
        "release belongs to original connection"
    );
    let revision = registry.revision();
    registry.detach(first);
    registry.detach(first);
    assert_eq!(registry.revision(), revision);
    assert_eq!(
        registry
            .current(&provider)
            .unwrap()
            .manifest()
            .registration_id,
        "c"
    );
    let current = registry.current(&provider).unwrap();
    registry.unregister(second, "c").unwrap();
    assert!(registry.current(&provider).is_none());
    assert!(registry.unregister(second, "c").is_err());
    assert!(out_two.try_recv().is_err());
    drop(current);
    release(out_two.try_recv().unwrap(), "c");
    registry.replace(second, manifest("c")).unwrap();
    registry.detach(second);
    assert!(registry.current(&provider).is_none());
    assert!(registry.published().next().is_none());
}

#[test]
fn authority_residency_and_reverse_backpressure_fail_closed() {
    let mut registry = Registry::default();
    let mut original = identity();
    original.principal_kind = PrincipalKind::CapabilityProvider;
    original.credential_bound_client_instance_id = Some("authenticated-desktop".into());
    original.capability_owner = Some(CapabilityOwnerIdentity {
        principal_id: "owner".into(),
        client_instance_id: "desktop".into(),
    });
    let id = Uuid::new_v4();
    let (endpoint, mut outbound) = Endpoint::channel(1);
    let closed = endpoint.closed();
    let provider = registry.attach(id, original.clone(), endpoint).unwrap();
    let mut invalid = manifest("invalid");
    invalid.offers[0].host_path_access = maka_runtime::capability::HostPathAccess::Cwd;
    assert!(registry.replace(id, invalid).is_err());
    assert_eq!(registry.revision(), 0);
    registry.replace(id, manifest("a")).unwrap();
    let snapshot = registry.current(&provider).unwrap();
    registry.detach(id);
    assert!(closed.is_cancelled());
    let mut forged = original.clone();
    forged.credential_bound_client_instance_id = None;
    assert!(
        registry
            .attach(Uuid::new_v4(), forged, Endpoint::channel(1).0)
            .is_err()
    );
    let mut forged = original.clone();
    forged.capability_owner.as_mut().unwrap().principal_id = "other".into();
    assert!(
        registry
            .attach(Uuid::new_v4(), forged, Endpoint::channel(1).0)
            .is_err()
    );
    assert!(
        outbound.try_recv().is_err(),
        "disconnected provider receives no release"
    );
    drop(snapshot);
    // A slow live peer cannot turn retired publications into unbounded memory.
    let next = Uuid::new_v4();
    let (endpoint, mut outbound) = Endpoint::channel(1);
    let closed = endpoint.closed();
    registry.attach(next, original, endpoint).unwrap();
    registry.replace(next, manifest("a")).unwrap();
    registry.replace(next, manifest("b")).unwrap();
    registry.replace(next, manifest("c")).unwrap();
    assert!(closed.is_cancelled());
    release(outbound.try_recv().unwrap(), "a");
    assert!(registry.current_for_connection(next).is_none());
    assert!(registry.replace(next, manifest("d")).is_err());
    registry.begin_drain();
    assert!(
        registry
            .attach(Uuid::new_v4(), identity(), Endpoint::channel(1).0)
            .is_err()
    );
}

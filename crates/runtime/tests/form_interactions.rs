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

use maka_runtime::{
    capability::{FormFieldSpec, FormInput, FormResult, FormValue},
    interaction::*,
};
use serde_json::{Value, json};
fn request() -> Value {
    json!({"kind":"form","toolUseId":"tool","message":"Line\n","requester":{"name":"Tool"},
    "fields":[{"kind":"integer","name":"count","label":"Count","required":true,"minimum":1,"maximum":3}]})
}
#[test]
fn canonical_forms_roundtrip_without_projection_and_bind_answers_to_fields() {
    let request: InteractionRequest = serde_json::from_value(request()).unwrap();
    assert_eq!(serde_json::to_value(&request).unwrap()["message"], "Line\n");
    for (values, valid) in [
        (json!({"count":2}), true),
        (json!({}), false),
        (json!({"count":2.5}), false),
        (json!({"count":4}), false),
        (json!({"count":2,"unknown":1}), false),
    ] {
        let answer: InteractionAnswer =
            serde_json::from_value(json!({"kind":"form","action":"accept","values":values}))
                .unwrap();
        assert_eq!(answer.validate_for_request(&request).is_ok(), valid);
        let outcome = answer.clone().into_outcome(7);
        assert_eq!(outcome.validate_for_request(&request).is_ok(), valid);
        assert!(answer.matches_outcome(&outcome));
        assert_eq!(
            serde_json::from_value::<InteractionOutcome>(serde_json::to_value(&outcome).unwrap())
                .unwrap(),
            outcome
        );
    }
    for action in ["decline", "cancel"] {
        let answer: InteractionAnswer =
            serde_json::from_value(json!({"kind":"form","action":action})).unwrap();
        answer.validate_for_request(&request).unwrap();
    }
}
#[test]
fn closed_shapes_and_public_nonfinite_construction_are_rejected() {
    for value in [
        json!({"kind":"form","action":"cancel","values":{}}),
        json!({"kind":"form","action":"accept","values":{"n":null}}),
        json!({"kind":"form","action":"accept","values":{},"extra":1}),
    ] {
        assert!(serde_json::from_value::<InteractionAnswer>(value).is_err());
    }
    let mut bad = request();
    bad["fields"][0]["unknown"] = json!(1);
    assert!(serde_json::from_value::<InteractionRequest>(bad).is_err());
    let mut bad = request();
    bad["requester"]["source"] = Value::Null;
    assert!(serde_json::from_value::<InteractionRequest>(bad).is_err());
    let mut input: FormInput =
        serde_json::from_value(json!({"message":"Test","requester":{"name":"Tool"},
        "fields":[{"kind":"number","name":"n","label":"N","required":true}]}))
        .unwrap();
    if let FormFieldSpec::Number { minimum, .. } = &mut input.fields[0].spec {
        *minimum = Some(f64::NAN);
    }
    assert!(input.validate().is_err());
    for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(
            FormResult::Accept {
                values: [("n".into(), FormValue::Number(number))].into()
            }
            .validate()
            .is_err()
        );
    }
}

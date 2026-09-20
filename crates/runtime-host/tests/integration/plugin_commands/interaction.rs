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

use super::super::support::peer::Peer;
use maka_plugins::execution::{CommandError, Commands, OfferInteraction, Prompt};
use maka_runtime::{
    event::Invocation,
    interaction::{InteractionOutcome, InteractionQuestion, InteractionRecord, QuestionOption},
};
use serde_json::json;

pub(super) async fn offer(
    commands: &dyn Commands,
    invocation: Invocation,
) -> (OfferInteraction, InteractionRecord) {
    let offer = OfferInteraction {
        operation_id: "choose-route".into(),
        invocation,
        prompt: Prompt::Question {
            questions: vec![InteractionQuestion {
                question: "Which route?".into(),
                options: ["A", "B"]
                    .map(|label| QuestionOption {
                        label: label.into(),
                        description: None,
                    })
                    .into(),
            }],
        },
    };
    let record = commands.offer_interaction(offer.clone()).await.unwrap();
    assert_eq!(
        commands.offer_interaction(offer.clone()).await.unwrap(),
        record
    );
    let mut conflicting = offer.clone();
    conflicting.prompt = Prompt::Question {
        questions: vec![InteractionQuestion {
            question: "Changed route?".into(),
            options: ["A", "B"]
                .map(|label| QuestionOption {
                    label: label.into(),
                    description: None,
                })
                .into(),
        }],
    };
    assert!(matches!(
        commands.offer_interaction(conflicting).await,
        Err(CommandError::Conflict)
    ));
    let mut denied = offer.clone();
    denied.operation_id = "foreign".into();
    denied.invocation.session_id = "ungranted".into();
    assert!(matches!(
        commands.offer_interaction(denied).await,
        Err(CommandError::Denied)
    ));
    let mut waiting = commands.wait_interaction(offer.operation_id.clone());
    assert!(futures_util::poll!(&mut waiting).is_pending());
    drop(waiting);
    assert_eq!(
        commands
            .interaction(offer.operation_id.clone())
            .await
            .unwrap(),
        Some(record.clone())
    );
    (offer, record)
}

pub(super) async fn answer(peer: &mut Peer, (_, record): &(OfferInteraction, InteractionRecord)) {
    let answered = peer
        .rpc(
            "interaction.answer",
            json!({
                "sessionId": record.session_id, "interactionId": record.request_id,
                "answer": { "kind": "question", "answers": ["A"] }
            }),
        )
        .await;
    assert_eq!(answered["ok"], true, "{answered}");
}

pub(super) async fn replay(
    commands: &dyn Commands,
    (offer, record): &(OfferInteraction, InteractionRecord),
) {
    let restored = commands.offer_interaction(offer.clone()).await.unwrap();
    assert_eq!(restored.request_id, record.request_id);
    assert!(
        matches!(&restored.outcome, Some(InteractionOutcome::QuestionAnswer { answers, .. })
        if answers == &vec![Some("A".into())])
    );
    assert_eq!(
        commands
            .wait_interaction(offer.operation_id.clone())
            .await
            .unwrap(),
        restored.outcome.clone().unwrap()
    );
    assert_eq!(
        commands
            .close_interaction(offer.operation_id.clone())
            .await
            .unwrap(),
        restored
    );
}

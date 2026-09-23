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

use super::*;
use crate::transcript::decode_session_transcript_page_input;
use crate::{Operation, OperationErrorCode as Code};

pub fn decode_input(operation: Operation, value: &Value) -> Result<Value> {
    match operation {
        Operation::SubscriptionOpen => {
            decode_subscription_open_input(value)?;
        }
        Operation::SubscriptionClose => {
            decode_subscription_close_input(value)?;
        }
        Operation::SubscriptionPtyInterestSet => {
            decode_pty_interest_input(value)?;
        }
        Operation::SessionTranscriptPage => {
            decode_session_transcript_page_input(value)?;
        }
        Operation::SessionTranscriptSearch => {
            crate::transcript::decode_transcript_search_input(value)?;
        }
        Operation::SubscriptionReady => {
            decode_subscription_close_input(value)?;
        }
        _ => {
            return Err(crate::ProtocolError::invalid(
                "Unsupported subscription operation",
            ));
        }
    }
    Ok(value.clone())
}

pub fn errors(operation: Operation) -> Option<&'static [Code]> {
    match operation {
        Operation::SessionTranscriptPage | Operation::SessionTranscriptSearch => Some(&[
            Code::HostNotReady,
            Code::HostDraining,
            Code::OperationUnavailable,
            Code::InvalidRequest,
            Code::NotFound,
            Code::OperationConflict,
            Code::PersistenceFailed,
            Code::InternalFailure,
        ]),
        Operation::SubscriptionOpen => Some(&[
            Code::TranscriptPreparing,
            Code::HostNotReady,
            Code::HostDraining,
            Code::OperationUnavailable,
            Code::NotFound,
            Code::OperationConflict,
            Code::PersistenceFailed,
            Code::InternalFailure,
        ]),
        Operation::SubscriptionClose
        | Operation::SubscriptionReady
        | Operation::SubscriptionPtyInterestSet => Some(&[
            Code::HostNotReady,
            Code::HostDraining,
            Code::OperationUnavailable,
            Code::NotFound,
            Code::InternalFailure,
        ]),
        _ => None,
    }
}

pub fn decode_output(operation: Operation, value: &Value) -> Result<Value> {
    match operation {
        Operation::SubscriptionOpen => {
            decode_subscription_open_result(value)?;
        }
        Operation::SessionTranscriptPage => {
            crate::transcript::decode_session_transcript_page(value)?;
        }
        Operation::SessionTranscriptSearch => {
            crate::transcript::decode_transcript_search_result(value)?;
        }
        Operation::SubscriptionReady
        | Operation::SubscriptionClose
        | Operation::SubscriptionPtyInterestSet => {
            decode_subscription_close_result(value)?;
        }
        _ => {
            return Err(crate::ProtocolError::invalid(
                "Unsupported subscription operation",
            ));
        }
    }
    Ok(value.clone())
}

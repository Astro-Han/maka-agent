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
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Position {
    pub position: u64,
    pub offset: Option<u64>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Claims {
    version: u8,
    binding: String,
    direction: SessionTranscriptPageDirection,
    through: Option<u64>,
    position: Position,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchClaims {
    binding: String,
    through: Option<u64>,
    query_digest: String,
    include_internal: bool,
    after: u64,
}
pub(super) struct Signer {
    key: [u8; 32],
    binding: String,
}
impl Signer {
    pub fn new(subscription: &str, session: &str) -> Self {
        let mut key = [0; 32];
        key[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        key[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        let mut hash = Sha256::new();
        hash.update((subscription.len() as u64).to_le_bytes());
        hash.update(subscription.as_bytes());
        hash.update(session.as_bytes());
        Self {
            key,
            binding: format!("{:x}", hash.finalize()),
        }
    }
    pub fn encode(&self, input: &SessionTranscriptPageInput, position: Position) -> Result<String> {
        self.seal(&Claims {
            version: 1,
            binding: self.binding.clone(),
            direction: input.direction,
            through: input.through_sequence,
            position,
        })
    }
    fn seal(&self, claims: &impl Serialize) -> Result<String> {
        let bytes = serde_json::to_vec(claims)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).expect("HMAC accepts 32-byte key");
        mac.update(&bytes);
        let token = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(&bytes),
            URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
        );
        if token.len() > SESSION_TRANSCRIPT_CURSOR_MAX_BYTES {
            return Err(TranscriptError::Capacity);
        }
        Ok(token)
    }
    pub fn decode(&self, input: &SessionTranscriptPageInput, token: &str) -> Result<Position> {
        let claims: Claims = self.open(token)?;
        if claims.version != 1
            || claims.binding != self.binding
            || claims.direction != input.direction
            || claims.through != input.through_sequence
        {
            return Err(TranscriptError::InvalidRequest(
                "invalid or mismatched cursor",
            ));
        }
        Ok(claims.position)
    }
    pub fn encode_search(&self, input: &TranscriptSearchInput, after: u64) -> Result<String> {
        self.seal(&SearchClaims {
            binding: self.binding.clone(),
            through: input.through_sequence,
            query_digest: format!("{:x}", Sha256::digest(input.query.as_bytes())),
            include_internal: input.include_internal,
            after,
        })
    }
    pub fn decode_search(&self, input: &TranscriptSearchInput, token: &str) -> Result<u64> {
        let claims: SearchClaims = self.open(token)?;
        if claims.binding != self.binding
            || claims.through != input.through_sequence
            || claims.query_digest != format!("{:x}", Sha256::digest(input.query.as_bytes()))
            || claims.include_internal != input.include_internal
        {
            return Err(TranscriptError::InvalidRequest(
                "invalid or mismatched search cursor",
            ));
        }
        Ok(claims.after)
    }
    fn open<T: serde::de::DeserializeOwned>(&self, token: &str) -> Result<T> {
        let invalid = || TranscriptError::InvalidRequest("invalid or mismatched cursor");
        if token.len() > SESSION_TRANSCRIPT_CURSOR_MAX_BYTES {
            return Err(invalid());
        }
        let (body, signature) = token.split_once('.').ok_or_else(invalid)?;
        let bytes = URL_SAFE_NO_PAD.decode(body).map_err(|_| invalid())?;
        let signature = URL_SAFE_NO_PAD.decode(signature).map_err(|_| invalid())?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).expect("HMAC accepts 32-byte key");
        mac.update(&bytes);
        mac.verify_slice(&signature).map_err(|_| invalid())?;
        serde_json::from_slice(&bytes).map_err(|_| invalid())
    }
}

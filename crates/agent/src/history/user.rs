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

use super::{images, references};
use crate::RunError;
use maka_model::prompt::Message;
use maka_runtime::{attachment::AttachmentKind, input::MessageInput};

/// The event identity owns the injection. Equal text in another user message
/// is never deduplicated or mistaken for a steering directive.
pub(super) fn project<'a>(
    content: &'a MessageInput,
    steering: bool,
    index: usize,
    images: &mut Vec<images::Target<'a>>,
    vision: bool,
) -> Result<Message, RunError> {
    if vision {
        images.extend(
            content
                .attachments
                .iter()
                .flatten()
                .filter(|image| image.kind == AttachmentKind::Image)
                .map(|image| images::Target::User {
                    message: index,
                    image,
                }),
        );
    }
    let text = references::text(content)?;
    let text = if steering {
        format!(
            "The user sent a message while you were working:\n<user_query>\n{text}\n</user_query>"
        )
    } else {
        text
    };
    Ok(Message::user(text))
}

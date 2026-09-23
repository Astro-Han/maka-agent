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

use super::{State, draft::Input};
use crate::pages::{references::Target, skills::Picked};

impl State {
    pub(crate) fn skills_source(&self) -> Option<&str> {
        Some(&self.saved.as_ref()?.copy.source_session_id)
    }
    fn skill_input(&self, session: &str, input: &str) -> Option<&Input> {
        self.saved
            .as_ref()
            .filter(|s| s.copy.target_session_id == session)?
            .inputs
            .iter()
            .find(|i| i.original.message_id == input)
    }
    pub(crate) fn skills(&self, session: &str, input: &str) -> Option<&[Picked]> {
        Some(&self.skill_input(session, input)?.skills)
    }
    pub(crate) fn skills_mut(&mut self, session: &str, input: &str) -> Option<&mut Vec<Picked>> {
        Some(
            &mut self
                .saved
                .as_mut()
                .filter(|s| s.copy.target_session_id == session)?
                .inputs
                .iter_mut()
                .find(|i| i.original.message_id == input)?
                .skills,
        )
    }
    pub(crate) fn skills_fit(&self, target: &Target, items: &[Picked]) -> bool {
        let Some(input) = &target.input else {
            return true;
        };
        let Some(input) = self.skill_input(&target.session, input) else {
            return false;
        };
        let mut input = input.clone();
        input.skills = items.to_vec();
        maka_runtime::input::validate_selections(&input.message().input_selections).is_ok()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::pages::{
        revision::{Command, Output},
        skills::{
            self,
            tests::{frame, page},
        },
    };
    #[test]
    fn revision_skill_selection_queries_source_but_belongs_to_one_new_input_and_checkpoint() {
        let (mut app, basis) = crate::pages::branch::tests::fixture();
        app.apply(Action::Revision(Command::Open(basis)));
        let request = app.revision_request().unwrap();
        app.revision_completed(
            request,
            Ok(Output::Sources(super::super::tests::sources("source"))),
        );
        frame(&mut app, 80, 26);
        app.apply(Action::Revision(Command::Select(1)));
        frame(&mut app, 80, 26);
        app.apply(Action::Revision(Command::Skills));
        let request = app.skills_request().unwrap();
        assert_eq!(request.session, "source");
        page(&mut app, request, 1, false);
        frame(&mut app, 80, 26);
        app.apply(Action::Skills(skills::Command::Toggle(0)));
        app.apply(Action::Skills(skills::Command::Close));
        assert!(app.revision.visible);
        let saved = app.revision.checkpoint().unwrap();
        saved.validate("root").unwrap();
        assert!(saved.inputs[0].skills.is_empty());
        assert_eq!(saved.inputs[1].skills[0].id, "review-000");
        assert!(
            !saved.inputs[1]
                .original
                .input_selections
                .contains_key(skills::PROVIDER)
        );
        assert_eq!(
            saved.inputs[1].message().input_selections[skills::PROVIDER],
            ["review-000"]
        );
        let mut invalid = saved.clone();
        invalid.inputs[1].skills[0].id.clear();
        assert!(invalid.validate("root").is_err());
        let mut changed = saved.clone();
        changed.inputs[1].skills = vec![
            Picked {
                id: "new".into(),
                name: "New".into()
            };
            51
        ];
        assert!(changed.validate("root").is_err());
        app.revision.restore(saved);
        app.apply(Action::Revision(Command::Resume));
        frame(&mut app, 80, 26);
        app.apply(Action::Revision(Command::Skills));
        let request = app.skills_request().unwrap();
        page(&mut app, request, 0, false);
        frame(&mut app, 52, 22);
        app.apply(Action::Skills(skills::Command::Selected));
        frame(&mut app, 52, 22);
        app.apply(Action::Skills(skills::Command::Toggle(0)));
        assert!(
            app.revision.checkpoint().unwrap().inputs[1]
                .skills
                .is_empty()
        );
    }
}

import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { renderGraphModePrompt } from '../graph-mode.js';

describe('Graph Mode shared prompt', () => {
  test('keeps the supervisor beside the data path and requires committed results', () => {
    const prompt = renderGraphModePrompt();
    assert.match(prompt, /main-agent supervisor beside the data path/);
    assert.match(prompt, /update_agent_graph/);
    assert.match(prompt, /agent_id.*implementation/);
    assert.match(prompt, /Use operator_id only.*existing runtime operator/);
    assert.match(prompt, /scheduling update must omit finish/);
    assert.match(prompt, /view_agent_graph/);
    assert.match(prompt, /committed record ids/);
    assert.match(prompt, /agent_output/);
    assert.match(prompt, /child_session_run.*childSessionId.*currentRunId/);
    assert.match(prompt, /Stay available to the user/);
  });
});

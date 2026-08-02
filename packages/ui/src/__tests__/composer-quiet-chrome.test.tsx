import { strict as assert } from 'node:assert';
import { describe, it } from 'node:test';
import type { ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { LocaleProvider } from '../locale-context.js';
import { Composer } from '../composer.js';

function render(children: ReactNode): string {
  return renderToStaticMarkup(<LocaleProvider locale="zh">{children}</LocaleProvider>);
}

describe('composer quiet chrome', () => {
  it('keeps resting chrome to permission icon, plus menu, model, and send', () => {
    const markup = render(
      <Composer
        onSend={() => true}
        onStop={() => {}}
        onPickAttachments={() => {}}
        onPermissionModeChange={() => {}}
        onPlanModeChange={() => {}}
        onSwarmModeChange={() => {}}
        mentionSkills={[{ id: 'pdf', name: 'PDF' }]}
        modelLabel="demo-model"
        modelChoices={[]}
      />,
    );

    // Quiet surface: no header attach/voice cluster, no standalone modes/skills triggers.
    assert.doesNotMatch(markup, /maka-composer-header-actions/);
    assert.doesNotMatch(markup, /maka-composer-header-context/);
    assert.doesNotMatch(markup, /maka-composer-modes-menu/);
    assert.doesNotMatch(markup, /maka-composer-skill-trigger/);
    assert.doesNotMatch(markup, /maka-composer-streaming-hint/);

    // Footer left: plus → permission → model (+ thinking when levels offered).
    // Footer right: send only (no mic by default).
    assert.match(markup, /permissionModeIcon/);
    assert.match(markup, /maka-composer-plus-menu/);
    assert.match(markup, /maka-composer-left-controls/);
    assert.match(markup, /maka-composer-right-controls/);
    const plusIdx = markup.indexOf('maka-composer-plus-menu');
    const permissionIdx = markup.indexOf('permissionModeIcon');
    assert.ok(plusIdx >= 0 && permissionIdx > plusIdx, 'plus must sit left of permission');
    // Model chip/switcher is left of send (in left-controls), not next to send.
    const leftControls = markup.match(
      /maka-composer-left-controls[\s\S]*?maka-composer-right-controls/,
    )?.[0] ?? '';
    assert.match(
      leftControls,
      /maka-composer-model-chip|maka-model-switcher|maka-new-chat-model-selector|maka-model-picker-root/,
      'model control must render in left-controls after permission',
    );
    // Thinking stays out of the model menu; without levels it does not mount.
    assert.doesNotMatch(markup, /maka-thinking-level-selector/);
    assert.doesNotMatch(markup, /maka-composer-voice-button/);
    assert.doesNotMatch(markup, /maka-composer-realtime-voice-button/);
  });

  it('places thinking beside the model in left-controls when levels are offered', () => {
    const markup = render(
      <Composer
        onSend={() => true}
        onStop={() => {}}
        modelLabel="demo-model"
        modelChoices={[]}
        newChatThinkingLevels={['off', 'low', 'medium', 'high']}
        newChatThinkingLevel="medium"
        onNewChatThinkingLevelChange={() => {}}
      />,
    );

    const leftControls = markup.match(
      /maka-composer-left-controls[\s\S]*?maka-composer-right-controls/,
    )?.[0] ?? '';
    assert.match(
      leftControls,
      /maka-model-selection-controls/,
      'model + thinking share one left-footer pair',
    );
    assert.match(
      leftControls,
      /maka-thinking-level-selector/,
      'thinking must sit beside the model in left-controls',
    );
    const rightControls = markup.match(
      /maka-composer-right-controls[\s\S]*$/,
    )?.[0] ?? '';
    assert.doesNotMatch(
      rightControls,
      /maka-thinking-level-selector/,
      'thinking must not sit next to send',
    );
  });

  it('reads active modes off the footer toolbar, after the model pair', () => {
    const markup = render(
      <Composer
        onSend={() => true}
        onStop={() => {}}
        planModeActive
        onPlanModeChange={() => {}}
        swarmModeActive
        onSwarmModeChange={() => {}}
        modelLabel="demo"
        modelChoices={[]}
        newChatThinkingLevels={['off', 'high']}
        newChatThinkingLevel="high"
        onNewChatThinkingLevelChange={() => {}}
      />,
    );

    const leftControls = markup.match(
      /maka-composer-left-controls[\s\S]*?maka-composer-right-controls/,
    )?.[0] ?? '';
    assert.match(leftControls, /maka-composer-mode-indicator[^>]*data-mode="plan"/);
    assert.match(leftControls, /maka-composer-mode-indicator[^>]*data-mode="swarm"/);
    // Modes trail the model + thinking pair so toggling one never shifts them.
    const modelIdx = leftControls.indexOf('maka-model-selection-controls');
    const modeIdx = leftControls.indexOf('maka-composer-mode-indicator');
    assert.ok(modelIdx >= 0 && modeIdx > modelIdx, 'modes must follow the model pair');
    // The mark is the same ghost icon button as ＋ and permission, named by
    // its mode and identified by its icon — never by a hue.
    const planMark = leftControls.slice(
      leftControls.indexOf('data-mode="plan"'),
      leftControls.indexOf('data-mode="swarm"'),
    );
    assert.match(planMark, /lucide-list-todo/, 'the icon says which mode it is');
    assert.match(planMark, /aria-label="Plan"/, 'the button is named for the mode');
    assert.match(planMark, /maka-composer-mode-button/, 'the way out stays reachable');
    assert.doesNotMatch(planMark, /astryx-token/, 'a mode is not a coloured pill');
    // Modes alone stage nothing for the next send, so no drawer mounts.
    assert.doesNotMatch(markup, /maka-composer-context-drawer/);
  });

  it('counts only send-consumed context in the drawer badge, never modes', () => {
    const markup = render(
      <Composer
        onSend={() => true}
        onStop={() => {}}
        planModeActive
        onPlanModeChange={() => {}}
        swarmModeActive
        onSwarmModeChange={() => {}}
        pendingAttachments={[{ displayName: 'notes.md', kind: 'doc', size: 12 }]}
        onRemoveAttachment={() => {}}
        modelLabel="demo"
      />,
    );

    assert.match(markup, /maka-composer-context-drawer/);
    // One attachment staged, two modes on — the badge reads 1.
    const badge = markup.match(/astryx-badge[\s\S]*?<\/span>/)?.[0] ?? '';
    assert.match(badge, />1</, `drawer badge must exclude modes, got: ${badge}`);
    // The drawer itself carries no mode marks any more.
    const drawer = markup.match(/maka-composer-context-drawer[\s\S]*?<\/div>/)?.[0] ?? '';
    assert.doesNotMatch(drawer, /maka-composer-mode-indicator/);
  });

  it('shows a single voice control only when the host wires capture', () => {
    const markup = render(
      <Composer
        onSend={() => true}
        onStop={() => {}}
        onToggleVoiceCapture={() => {}}
        modelLabel="demo"
      />,
    );
    assert.match(markup, /maka-composer-voice-button/);
    assert.doesNotMatch(markup, /maka-composer-realtime-voice-button/);
  });

  it('places the workspace picker above the composer card for new chats', () => {
    const markup = render(
      <Composer
        onSend={() => true}
        onStop={() => {}}
        modelLabel="demo"
        workspacePicker={{
          projects: [],
          onAdd: () => {},
          onSelectProject: () => {},
          onRelink: () => {},
          onSelectNoProject: () => {},
        }}
      />,
    );
    assert.match(markup, /maka-composer-workspace-dock/);
    // Workspace row is a sibling above the form, not nested after the card.
    const dockIdx = markup.indexOf('maka-composer-workspace-dock');
    const formIdx = markup.indexOf('maka-composer composer');
    assert.ok(dockIdx >= 0 && formIdx > dockIdx);
  });

  /**
   * The project and branch decide where a NEW chat starts; once a session
   * exists they no longer move it. Leaving the row on screen in an open chat
   * reads as "you can still change this session's context here", which is
   * false — so the same wired picker must render nothing.
   */
  it('drops the workspace picker once a session is active', () => {
    const workspacePicker = {
      projects: [],
      onAdd: () => {},
      onSelectProject: () => {},
      onRelink: () => {},
      onSelectNoProject: () => {},
    };
    const markup = render(
      <Composer
        onSend={() => true}
        onStop={() => {}}
        modelLabel="demo"
        workspacePicker={workspacePicker}
        activeSession={{
          id: 'session-1',
          name: 'Test',
          isFlagged: false,
          isArchived: false,
          labels: [],
          hasUnread: false,
          status: 'done',
          backend: 'fake',
          llmConnectionSlug: 'fake',
          connectionLocked: false,
          model: 'fake',
          permissionMode: 'ask',
        }}
      />,
    );
    assert.doesNotMatch(markup, /maka-composer-workspace-dock/);
  });
});

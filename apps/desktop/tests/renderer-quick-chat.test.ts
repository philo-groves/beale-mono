import { readFileSync } from 'node:fs';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { QuickChatDock } from '../src/renderer/features/quick-chat/QuickChatDock';

describe('quick chats', () => {
  it('places Quick Chat at the top of the utility actions above Automations', () => {
    const source = readFileSync(new URL('../src/renderer/features/workspaces/WorkspaceSidebar.tsx', import.meta.url), 'utf8');
    const utilityActions = source.indexOf('className="sidebar-quick-actions"');
    const quickChat = source.indexOf('className="sidebar-utility-button sidebar-quick-chat"');
    const automations = source.indexOf('title="Automations"');
    expect(utilityActions).toBeGreaterThan(-1);
    expect(quickChat).toBeGreaterThan(utilityActions);
    expect(automations).toBeGreaterThan(quickChat);
    expect(source.slice(quickChat, automations)).toContain('<Zap size={15} />');
    expect(source.slice(quickChat, automations)).toContain('<span>New Quick Chat</span>');
  });

  it('renders fixed bottom-right cards that stack toward the left and expose resize, collapse, and close actions', () => {
    const component = readFileSync(new URL('../src/renderer/features/quick-chat/QuickChatDock.tsx', import.meta.url), 'utf8');
    const styles = readFileSync(new URL('../src/renderer/styles.css', import.meta.url), 'utf8');
    expect(component).toContain("'Collapse quick chat'");
    expect(component).toContain('aria-label="Resize quick chat"');
    expect(component).toContain('onPointerDown={beginResize}');
    expect(component).not.toContain('MoveDiagonal2');
    expect(component).toContain('bounds.width + startX - moveEvent.clientX');
    expect(component).toContain('bounds.height + startY - moveEvent.clientY');
    expect(component).toContain('aria-label="Close quick chat"');
    expect(component).toContain('QUICK_CHAT_EXIT_ANIMATION_MS = 180');
    expect(component).toContain("exiting ? ' is-exiting' : ''");
    expect(component).toContain('current.filter((entry) => !entry.exiting)');
    expect(component).toContain('window.beale.startQuickChat');
    expect(component).toContain('showCollaboration={false}');
    expect(component).toContain('initialSafetyMode="auto_review"');
    expect(component).toContain('showSafetyMode={false}');
    expect(component).toContain("onReportInitialInstruction(instruction, modelSelection, 'auto_review')");
    expect(component).not.toContain('safetyModeOptions=');
    expect(component).toContain('className="quick-chat-empty-state"');
    expect(component).toContain('This chat will be automatically archived.');
    expect(component).toContain('<BealeWelcomeIcon />');
    expect(component).toContain("reportEditing ? 'Describe a change to this report' : 'Any topic, any workspace'");
    expect(styles).toMatch(/\.quick-chat-dock\s*\{[^}]*position:\s*fixed;[^}]*right:\s*14px;[^}]*bottom:\s*38px;/s);
    expect(styles).toMatch(/\.quick-chat-dock\s*\{[^}]*flex-direction:\s*row-reverse;/s);
    expect(styles).toMatch(/\.quick-chat-dock\s*\{[^}]*overflow:\s*visible;/s);
    expect(styles).toMatch(/\.quick-chat-card\s*\{[^}]*width:\s*min\(340px,/s);
    expect(styles).toMatch(/\.quick-chat-card\s*\{[^}]*background:\s*var\(--panel-raised\);/s);
    expect(styles).toMatch(/\.quick-chat-card\s*\{[^}]*transform-origin:\s*bottom right;[^}]*animation:\s*quick-chat-enter 180ms/s);
    expect(styles).toMatch(/\.quick-chat-card\.is-exiting\s*\{[^}]*pointer-events:\s*none;[^}]*animation:\s*quick-chat-exit 180ms/s);
    expect(styles).toContain('@keyframes quick-chat-enter');
    expect(styles).toContain('@keyframes quick-chat-exit');
    expect(styles).toMatch(/\.quick-chat-resize-anchor\s*\{[^}]*position:\s*absolute;[^}]*top:\s*1px;[^}]*left:\s*1px;/s);
    expect(styles).toMatch(/\.quick-chat-resize-anchor\s*\{[^}]*width:\s*10px;[^}]*height:\s*10px;/s);
    expect(styles).toMatch(/\.quick-chat-header\s*\{[^}]*padding:\s*0 10px;/s);
    expect(styles).toMatch(/\.quick-chat-header > span\s*\{[^}]*font-size:\s*1rem;[^}]*font-weight:\s*400;/s);
    expect(styles).toMatch(/\.quick-chat-header\s*\{[^}]*border-bottom:\s*0;/s);
    expect(styles).not.toMatch(/\.quick-chat-body \.main-trace-footer\s*\{[^}]*--trace-footer-radius:/s);
    expect(styles).toMatch(/\.quick-chat-body \.main-commentary-scroll\s*\{[^}]*width:\s*calc\(100% - 10px\);[^}]*margin:\s*2px 5px 1px;[^}]*border-radius:\s*10px;[^}]*background:\s*var\(--panel\);/s);
    expect(styles).toMatch(/\.quick-chat-body \.main-commentary-list\s*\{[^}]*padding:\s*10px 5px;/s);
    expect(styles).toMatch(/\.quick-chat-empty-state\s*\{[^}]*place-content:\s*center;[^}]*justify-items:\s*center;/s);
    expect(styles).toMatch(/\.quick-chat-empty-state \.new-research-welcome-icon\s*\{[^}]*width:\s*80px;[^}]*height:\s*80px;[^}]*margin:\s*0;/s);
    expect(styles).toMatch(/\.quick-chat-empty-state p\s*\{[^}]*font-size:\s*1rem;/s);
    expect(styles).toMatch(/\.quick-chat-body \.main-trace-footer\s*\{[^}]*--trace-footer-composer-inset:\s*2px;[^}]*--trace-footer-composer-bottom-inset:\s*0px;[^}]*background:\s*var\(--panel-raised\);/s);
    expect(styles).toMatch(/\.quick-chat-body \.main-steer-input-row\s*\{[^}]*border-radius:\s*16px;[^}]*corner-shape:\s*squircle;/s);
    expect(styles).toMatch(/\.quick-chat-body \.main-steer-input-row\.without-trace-filters \.main-steer-model-selection-picker\s*\{[^}]*margin-left:\s*8px;/s);
    expect(styles).not.toMatch(/\.quick-chat-body \.main-steer-input-row\s*\{[^}]*border:/s);
    expect(styles).toMatch(/\.quick-chat-body \.main-steer-input-row textarea\s*\{[^}]*border-radius:\s*16px 16px 0 0;[^}]*corner-shape:\s*squircle;[^}]*padding:\s*12px 8px;/s);
    expect(styles).toMatch(/\.quick-chat-body \.main-steer-input-row :is\([^)]+\.main-steer-model-selection-picker,[^)]+\.main-steer-context-usage,[^)]+\.main-steer-send[^)]*\)\s*\{[^}]*transform:\s*translateY\(10px\);/s);
  });

  it('renders report editing as a labeled dock card without a close action', () => {
    const html = renderToStaticMarkup(createElement(QuickChatDock, {
      chats: [{ id: 'report-edit:workspace:report', kind: 'report-edit', title: 'Parser boundary confusion' }],
      providerModelCatalog: [],
      onClose: () => undefined,
      onRunStarted: () => undefined
    }));

    expect(html).toContain('class="quick-chat-card is-report-edit"');
    expect(html).toContain('Parser boundary confusion');
    expect(html).toContain('lucide-file-text');
    expect(html).not.toContain('Report editing ·');
    expect(html).not.toContain('lucide-message-square');
    expect(html).toContain('Review and improve this report.');
    expect(html).toContain('This chat will be automatically archived.');
    expect(html).toContain('class="new-research-welcome-icon"');
    expect(html).toContain('Send a report editing message');
    expect(html).not.toContain('Safety');
    expect(html).not.toContain('aria-label="Close quick chat"');
  });

  it('bundles explicit current-or-selected workspace query and edit tools', () => {
    const server = readFileSync(new URL('../../../app-server/resources/agent-plugins/beale-introspection/server.mjs', import.meta.url), 'utf8');
    expect(server).toContain("name: 'get_workspace'");
    expect(server).toContain("name: 'edit_workspace'");
    expect(server).toContain('current or selected registered Beale workspace');
  });

  it('waits for the lead provider preferred model before enabling the composer catalog', () => {
    const app = readFileSync(new URL('../src/renderer/App.tsx', import.meta.url), 'utf8');
    expect(app).toContain('quickChats.length === 0');
    expect(app).toContain('openAiStatus,');
    expect(app).toContain('researchProviderStatuses');
    expect(app).toContain('if (!preferredModel) return null;');
    expect(app).toContain('providerModelCatalog={quickChatInitialModelSelection ? enabledResearchProviderModelCatalog : []}');
    expect(app).toContain('initialModelSelection={quickChatInitialModelSelection ?? undefined}');
  });
});

import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { WorkspaceRegistry } from '../../../app-server/src/workspaceRegistryStore';
import { AppServerHostRegistry } from '../../../app-server/src/hostRegistry';
import { appServerSessionArgs } from '../../../app-server/src/sessionLaunch';
import { invokeAppServerProtocol } from '../../../app-server/src/appServerProtocolClient';

describe('prompt template settings', () => {
  it('persists a profile override, previews it, and restores the default', () => {
    const directory = mkdtempSync(join(tmpdir(), 'beale-prompt-template-'));
    try {
      const registry = new WorkspaceRegistry(directory);
      const initial = registry.getPromptTemplateSettings('security-research');
      expect(initial.overridden).toBe(false);
      const template = '{{boundary}}\nExample prompt for {{profile.name}}.\n{{identity}}';
      registry.setPromptTemplate('security-research', template);
      expect(() => registry.getPromptTemplateSettings('mathematics')).toThrow(/Unsupported active research profile/);
      expect(registry.previewPromptTemplate('security-research', template)).toContain('Example prompt for Security.');
      const pluginPreview = registry.previewPromptTemplate('security-research', '{{boundary}}\n{{plugins}}', undefined, [{
        id: 'example-plugin', name: 'Example Plugin', mcpServers: [],
        skills: [{ id: 'example-skill', name: 'Example Skill', useWhen: 'Inspect synthetic records.', path: '',
          resourceCounts: { scripts: 1, references: 2, assets: 0 } }]
      }]);
      expect(pluginPreview).toContain('example-plugin (plugin; 0 tools, 1 skill):');
      expect(pluginPreview).not.toContain('example-skill');
      expect(pluginPreview).not.toContain('Inspect synthetic records.');
      expect(pluginPreview).not.toContain('Example Plugin');
      expect(pluginPreview).not.toContain('Example Skill');
      const conditionalTemplate = '{{boundary}}\n{{memory}}\n{{claims}}\n{{runbooks}}\n{{reports}}';
      const knowledgeOnly = registry.previewPromptTemplate('security-research', conditionalTemplate, undefined, [{
        id: 'beale-knowledge', name: 'Knowledge', mcpServers: [], skills: []
      }]);
      expect(knowledgeOnly).toContain('Use durable memory as a concise research graph');
      expect(knowledgeOnly).not.toContain('Use one canonical, evidence-gated research claim ledger');
      expect(knowledgeOnly).not.toContain('Use runbooks as durable executable research artifacts');
      expect(knowledgeOnly).not.toContain('Use reports as durable Markdown artifacts');
      registry.close();

      const host = new AppServerHostRegistry({ registryDirectory: directory });
      expect(host.promptTemplateOverride('security-research')).toBe(template);
      const args = appServerSessionArgs({
        workspaceRoot: directory,
        workspaceDirectories: [directory],
        capturePath: join(directory, 'capture.json'),
        attemptId: 'attempt-example',
        promptMarkdown: 'Example request',
        provider: { id: 'openai-codex', riskAcknowledgements: [], authenticationPreferences: {} },
        shellSafetyMode: 'auto_review',
        promptTemplatePath: join(directory, 'prompt-template.txt'),
        pluginRuntime: { managedPluginIds: ['beale-knowledge'], pluginCatalogPath: join(directory, 'plugin-catalog.json') },
        profileAware: true,
        memoryBackend: 'disabled',
        storage: { databasePath: join(directory, 'memory.sqlite'), artifactDirectoryPath: directory }
      }, {});
      expect(args.slice(args.indexOf('--prompt-template-file'), args.indexOf('--prompt-template-file') + 2)).toEqual([
        '--prompt-template-file', join(directory, 'prompt-template.txt')
      ]);
      expect(args.slice(args.indexOf('--plugin-catalog'), args.indexOf('--plugin-catalog') + 2)).toEqual([
        '--plugin-catalog', join(directory, 'plugin-catalog.json')
      ]);

      const reopened = new WorkspaceRegistry(directory);
      expect(reopened.getPromptTemplateSettings('security-research')).toMatchObject({ template, overridden: true });
      expect(() => reopened.setPromptTemplate('security-research', '{{identity}}')).toThrow(/boundary/);
      reopened.resetPromptTemplate('security-research');
      expect(reopened.getPromptTemplateSettings('security-research')).toEqual(initial);
      expect(host.promptTemplateOverride('security-research')).toBeNull();
      reopened.close();
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });

  it('exposes preview and save through the app-server registry operation', async () => {
    const directory = mkdtempSync(join(tmpdir(), 'beale-prompt-operation-'));
    try {
      const template = '{{boundary}}\nExample {{profile.id}}';
      await invokeAppServerProtocol('registry.state', {
        args: [], input: { registryDirectory: directory, action: 'setPromptTemplate', args: ['security-research', template] }
      });
      const preview = await invokeAppServerProtocol<string>('registry.state', {
        args: [], input: { registryDirectory: directory, action: 'previewPromptTemplate', args: ['security-research', template] }
      });
      expect(preview).toContain('Example security-research');
      await invokeAppServerProtocol('registry.state', {
        args: [], input: { registryDirectory: directory, action: 'resetPromptTemplate', args: ['security-research'] }
      });
      const settings = await invokeAppServerProtocol<{ overridden: boolean }>('registry.state', {
        args: [], input: { registryDirectory: directory, action: 'getPromptTemplateSettings', args: ['security-research'] }
      });
      expect(settings.overridden).toBe(false);
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });

});

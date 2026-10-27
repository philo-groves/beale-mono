import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const sections = JSON.parse(readFileSync(new URL('./content.json', import.meta.url), 'utf8'));
const expected = new Map([
  ['start-here', ['overview']],
  ['architecture', ['app-server', 'desktop-agent', 'ios-agent']],
  ['ways-to-work', ['sessions', 'goal-mode', 'quick-chats', 'automations', 'tracks', 'fleet', 'collaboration']],
  ['workspaces', ['rules-settings', 'resources', 'surfaces', 'targets', 'research-kits', 'agents-md']],
  ['information', ['findings', 'memories', 'reports', 'runbooks']],
  ['agent-settings', ['permissions', 'suggestions', 'appearance', 'providers', 'prompt']],
  ['other-features', ['web-browser', 'archives', 'plugins']]
]);

assert.equal(sections.length, expected.size);
const routes = new Set(['/docs/', '/downloads/']);
for (const section of sections) {
  assert.ok(expected.has(section.slug), `unexpected section ${section.slug}`);
  assert.ok(section.title && section.summary);
  assert.deepEqual(section.pages.map((page) => page.slug), expected.get(section.slug));
  routes.add(`/docs/${section.slug}/`);
  for (const page of section.pages) {
    assert.ok(page.title && page.summary && page.blocks.length > 0, `empty guide ${page.slug}`);
    routes.add(`/docs/${section.slug}/${page.slug}/`);
    for (const block of page.blocks) {
      assert.ok(block.heading || block.paragraphs?.length, `empty block in ${page.slug}`);
    }
  }
}
for (const section of sections) {
  for (const page of section.pages) {
    for (const block of page.blocks) {
      for (const link of block.links ?? []) {
        assert.ok(link.label && link.url);
        if (link.url.startsWith('/')) assert.ok(routes.has(link.url), `broken link ${link.url}`);
      }
    }
  }
}
const ios = sections.find((section) => section.slug === 'architecture').pages.find((page) => page.slug === 'ios-agent');
assert.match(JSON.stringify(ios), /not available for download on the iOS App Store/);
console.log(`Validated ${[...routes].filter((route) => route.startsWith('/docs/')).length} handbook routes.`);

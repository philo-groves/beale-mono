export interface BrowserContextSummary {
  id: string;
  label: string;
  partition: string;
  lastUrl: string;
}

export interface BrowserContextsUpdate {
  contexts: BrowserContextSummary[];
  createdId?: string;
  removedId?: string;
  openedId?: string;
}

export const DEFAULT_BROWSER_CONTEXT: BrowserContextSummary = {
  id: 'default',
  label: 'Default',
  partition: browserContextPartition('default'),
  lastUrl: 'about:blank'
};

export const MAX_BROWSER_CONTEXTS = 12;

export function browserContextPartition(id: string): string {
  return `persist:beale-in-agent-browser-${id}`;
}

export function browserContextLabel(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  const label = value.trim();
  return /^[A-Za-z0-9_][A-Za-z0-9_-]{0,23}$/u.test(label) ? label : null;
}

export function browserSideViewId(contextId: string): `browser:${string}` {
  return `browser:${contextId}`;
}

export function isBrowserSideView(view: string): view is `browser:${string}` {
  return view.startsWith('browser:');
}

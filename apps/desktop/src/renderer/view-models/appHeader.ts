export function displayWorkspaceHeaderName(workspaceName: string | null | undefined): string {
  const normalized = (workspaceName ?? '').trim().replace(/\s+/g, ' ');
  if (!normalized) return 'No Workspace Selected';
  return normalized;
}

export function displayChannelTitle(title: string | null | undefined): string {
  const normalized = (title ?? '')
    .trim()
    .toLocaleLowerCase()
    .replace(/[^a-z0-9]+/gu, '-')
    .replace(/^-+|-+$/gu, '');
  if (!normalized) return 'Channel';
  return normalized;
}

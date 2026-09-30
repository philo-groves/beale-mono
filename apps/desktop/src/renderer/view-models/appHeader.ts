export function displayWorkspaceHeaderName(workspaceName: string | null | undefined): string {
  const normalized = (workspaceName ?? '').trim().replace(/\s+/g, ' ');
  if (!normalized) return 'No Workspace Selected';
  return normalized;
}

export function displayTopicTitle(title: string | null | undefined): string {
  const normalized = (title ?? '').trim().replace(/\s+/gu, ' ');
  if (!normalized) return 'Topic';
  return normalized;
}

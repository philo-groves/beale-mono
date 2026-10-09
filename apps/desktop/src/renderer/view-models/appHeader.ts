export function displayWorkspaceHeaderName(workspaceName: string | null | undefined): string {
  const normalized = (workspaceName ?? '').trim().replace(/\s+/g, ' ');
  if (!normalized) return 'No Workspace Selected';
  return normalized;
}

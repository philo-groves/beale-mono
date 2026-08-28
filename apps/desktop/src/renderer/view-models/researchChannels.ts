export const MAX_RESEARCH_CHANNEL_NAME_WORDS = 3;

export function normalizeResearchChannelNameDraft(value: string): string {
  const separatorAtEnd = /[^a-z0-9]$/iu.test(value);
  const words = value
    .toLocaleLowerCase()
    .match(/[a-z0-9]+/gu)
    ?.slice(0, MAX_RESEARCH_CHANNEL_NAME_WORDS) ?? [];
  const normalized = words.join('-');
  return separatorAtEnd && words.length > 0 && words.length < MAX_RESEARCH_CHANNEL_NAME_WORDS
    ? `${normalized}-`
    : normalized;
}

export function canonicalResearchChannelName(value: string): string {
  return normalizeResearchChannelNameDraft(value).replace(/-+$/gu, '');
}

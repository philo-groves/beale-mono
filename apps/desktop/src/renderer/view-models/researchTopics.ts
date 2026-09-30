export const MAX_RESEARCH_TOPIC_NAME_WORDS = 8;

export function normalizeResearchTopicNameDraft(value: string): string {
  const separatorAtEnd = /[^a-z0-9]$/iu.test(value);
  const words = value
    .toLocaleLowerCase()
    .match(/[a-z0-9]+/gu)
    ?.slice(0, MAX_RESEARCH_TOPIC_NAME_WORDS) ?? [];
  const normalized = words.join('-');
  return separatorAtEnd && words.length > 0 && words.length < MAX_RESEARCH_TOPIC_NAME_WORDS
    ? `${normalized}-`
    : normalized;
}

export function canonicalResearchTopicName(value: string): string {
  return normalizeResearchTopicNameDraft(value).replace(/-+$/gu, '');
}

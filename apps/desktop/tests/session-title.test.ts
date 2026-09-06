import { describe, expect, it } from 'vitest';
import { displaySessionTitle, generateSessionTitle, SESSION_TITLE_FALLBACK, SESSION_TITLE_MAX_WORDS } from '../src/shared/sessionTitle';

describe('session title generation', () => {
  it('generates a strict six-word maximum title from a research prompt', () => {
    const title = generateSessionTitle('Please perform deep vulnerability analysis on the Sample Parser open source library by ExampleCo.');

    expect(title.split(/\s+/).length).toBeLessThanOrEqual(SESSION_TITLE_MAX_WORDS);
    expect(title).toBe('Sample Parser Open Source Library Exampleco');
  });

  it('uses No Title Yet for overlong stored titles', () => {
    expect(displaySessionTitle("Let's perform deep vulnerability analysis on the Sample Parser open source library by ExampleCo")).toBe(SESSION_TITLE_FALLBACK);
  });

  it('prefers meaningful generated prompt headings over boilerplate sections', () => {
    const prompt = `## Next research session: ExampleCo Help Center first-party support/chat surface

### Target focus
Focus on the underexplored in-scope **Primary Target** \`support.example.test\`, especially first-party support flows, locale/account-help pages, contact-us/chat entry points, and first-party API calls.`;

    expect(generateSessionTitle(prompt)).toBe('Exampleco Help Center Support Chat');
    expect(displaySessionTitle('Target Focus Focus Underexplored In-Scope Primary', prompt)).toBe('Exampleco Help Center Support Chat');
  });

  it('unwraps generated-prompt JSON before deriving a recovery title', () => {
    const prompt = JSON.stringify({
      promptMarkdown: '# New Mathematics Research: Reduction and lifting in the Aster–Lumen conjecture\n\nInvestigate divisor lifting.'
    });

    expect(generateSessionTitle(prompt)).toBe('Reduction Lifting Aster Lumen Conjecture');
  });
});

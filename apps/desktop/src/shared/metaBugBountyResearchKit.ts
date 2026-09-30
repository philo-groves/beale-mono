import type { ScopeAssetInput } from './types';

export const META_BUG_BOUNTY_SCOPE_URL = 'https://bugbounty.meta.com/scope/';
export const META_BUG_BOUNTY_TERMS_URL = 'https://bugbounty.meta.com/terms/';
export const META_BUG_BOUNTY_PAYOUT_GUIDELINES_URL = 'https://bugbounty.meta.com/payout-guidelines/';

export const META_BUG_BOUNTY_RULES = [
  'Check the current Meta Bug Bounty terms, program scope, and payout guidelines before testing or reporting. The bundled scope resources are examples, not a complete authorization list.',
  'Test only your own account, a test account, or another account whose owner gave explicit written consent. Do not interact with another person\'s account or data without that consent.',
  'Make a good-faith effort to avoid privacy violations, unauthorized access or destruction of data, and disruption or degradation of Meta services. Do not perform denial-of-service testing.',
  'If you inadvertently access another person\'s or Meta company data, stop further access, promptly notify Meta with a description, delete the data from your system, do not share it, and acknowledge the access in any related report.',
  'Use a discovered issue only for testing. Do not expand testing beyond authorized accounts or use the issue to demonstrate access to other users\' accounts or sensitive company data.',
  'Give Meta reasonable time to investigate and mitigate a reported issue before public disclosure or sharing report details with others.',
  'Report a security or privacy risk promptly through Meta\'s vulnerability report form, with one issue per report, and respond to requests for follow-up information. Use test accounts when investigating; Meta permits an authorized real account when a test account cannot reproduce the issue, except for automated testing.',
  'Do not test third-party apps or websites under Meta\'s authorization. The narrow third-party reporting exception requires passive observation without manipulating requests, or separate authorization from that third party, plus potential impact on Meta user data or systems.',
  'Spam, social engineering, denial of service, and third-party issues outside Meta\'s stated exception are out of scope. Content injection needs a clearly demonstrated significant risk; script execution confined to sandboxed domains is not an in-scope Meta-origin issue.',
  'WhatsApp Private Processing security validation is invitation-only. Do not treat its mention in the scope or payout pages as testing authorization.',
  'The payout guidelines describe discretionary reward assessment, not additional testing authorization. Show a reproducible security or privacy impact and consult the category-specific guidance relevant to the report.'
] as const;

const ELIGIBLE_DOMAINS = [
  'facebook.com', 'fb.com', 'fb.me', 'thefacebook.com', 'm.facebook.com',
  'blog.whatsapp.com', 'translate.whatsapp.com', 'web.whatsapp.com', 'whatsapp.net', 'www.whatsapp.com',
  'instagram.com', 'threads.com', 'messenger.com', 'muse.ai', 'meta.ai',
  'meta.com', 'oculus.com', 'freebasics.com', 'mapillary.com', 'getsupernatural.com'
] as const;

const INELIGIBLE_DOMAINS = [
  'fbsbx.com', 'investor.fb.com', 'accountkit.com', 'alpha.whatsapp.com', 'media.whatsapp.com',
  'communityforums.atmeta.com', 'ray-ban.com', 'daytum.com', 'drop.io', 'face.com',
  'friendfeed.com', 'monoidics.com', 'opencompute.org', 'spaceport.io'
] as const;

const ELIGIBLE_SERVICES = [
  'Facebook', 'Facebook Lite', 'Meta Business Suite', 'Meta Ads Manager',
  'WhatsApp', 'WhatsApp Business', 'Instagram', 'Threads', 'Instagram Lite',
  'Boomerang', 'Hyperlapse', 'Layout', 'Messenger', 'Messenger Kids', 'Muse', 'Meta AI',
  'Meta Quest first-party hardware and apps',
  'Ray-Ban Meta first-party hardware, software, and Meta View', 'Free Basics', 'Mapillary',
  'Supernatural: Unreal Fitness', 'https://github.com/facebook/', 'https://github.com/facebookincubator/'
] as const;

function scopeExample(direction: ScopeAssetInput['direction'], kind: ScopeAssetInput['kind'], value: string): ScopeAssetInput {
  return {
    direction,
    kind,
    value,
    sensitivity: 'public',
    attributes: {
      source: 'meta-bug-bounty',
      researchKitId: 'meta-bug-bounty',
      researchKitSourceUrl: META_BUG_BOUNTY_SCOPE_URL,
      scopeExample: true
    }
  };
}

export const META_BUG_BOUNTY_SCOPE_ASSETS: readonly ScopeAssetInput[] = [
  ...ELIGIBLE_DOMAINS.map((value) => scopeExample('in_scope', 'domain', value)),
  ...ELIGIBLE_SERVICES.map((value) => scopeExample('in_scope', 'service', value)),
  ...INELIGIBLE_DOMAINS.map((value) => scopeExample('out_of_scope', 'domain', value)),
  scopeExample('out_of_scope', 'service', 'Third-party apps and websites without separate authorization'),
  scopeExample('out_of_scope', 'service', 'Non-Meta Ray-Ban hardware, software, and services'),
  scopeExample('out_of_scope', 'service', 'Facebook for Blackberry'),
  scopeExample('out_of_scope', 'service', 'Facebook for Windows'),
  scopeExample('out_of_scope', 'service', 'https://github.com/facebookarchive/')
];

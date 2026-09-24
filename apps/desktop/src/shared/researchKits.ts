import type { ResearchProfileId } from './researchProfile';
import type { ScopeAssetInput } from './types';
import { MSRC_WINDOWS_PROGRAM_URL, MSRC_WINDOWS_RESOURCES } from './msrcWindowsResearchKit';
import {
  META_BUG_BOUNTY_PAYOUT_GUIDELINES_URL,
  META_BUG_BOUNTY_RULES,
  META_BUG_BOUNTY_SCOPE_ASSETS,
  META_BUG_BOUNTY_SCOPE_URL,
  META_BUG_BOUNTY_TERMS_URL
} from './metaBugBountyResearchKit';
import {
  GOOGLE_OSS_REPOSITORIES,
  GOOGLE_OSS_RULES,
  GOOGLE_OSS_TIER_LIST_URL,
  type GoogleOssRepositoryTier
} from './googleOssResearchKit';

export type { GoogleOssRepositoryTier } from './googleOssResearchKit';

export const RESEARCH_KIT_IDS = ['general', 'hackerone', 'apple-security-bounty', 'google-oss-vrp', 'msrc', 'meta-bug-bounty'] as const;

export type ResearchKitId = typeof RESEARCH_KIT_IDS[number];

export type ResearchKitImportKind = 'resources' | 'rules' | 'guidance';

export interface ResearchKitRepositoryCatalogEntry {
  name: string;
  url: string;
  archived: boolean;
  tier?: GoogleOssRepositoryTier;
}

export interface ResearchKitDefinition {
  id: ResearchKitId;
  label: string;
  description: string;
  supportedResearchProfileIds: readonly ResearchProfileId[];
  scopeLookup?: 'hackerone';
  onboardingDefaults?: {
    workspaceName: string;
    researchSubjectName: string;
    descriptionMarkdown: string;
    rules: readonly string[];
    assets?: readonly ScopeAssetInput[];
  };
  repositoryCatalog?: ({
    provider: 'github-organization';
    organization: string;
  } | {
    provider: 'bundled';
    sourceUrl: string;
    repositories: readonly ResearchKitRepositoryCatalogEntry[];
  }) & { resourceSource: string };
  resourceCatalog?: {
    sourceUrl: string;
    resourceSource: string;
    resources: readonly ScopeAssetInput[];
  };
  refresh?: {
    sourceLabel: string;
    sourceDescription: string;
    sourceIdentifierPlaceholder?: string;
    fixedSource?: string;
    imports: readonly ResearchKitImportKind[];
  };
}

const APPLE_RULES = [
  'Verify current Apple Security Bounty scope, categories, guidelines, Target Flags, and submission requirements before testing or submitting.',
  'Provide a complete and actionable report with observed behavior, expected behavior, the security or privacy mechanism bypassed, and attacker impact.',
  'Include a reliable exploit or proof of concept, plus concise numbered reproduction steps.',
  'For zero-click, one-click, or multi-exploit issues, submit the full chain as one report with everything needed to execute it and a nondestructive payload when needed.',
  'Include crash logs, sysdiagnose output, or video demonstrations when applicable.',
  'Use Target Flags when they apply to the category or reward level. For kernel or user-level privilege escalation, include a Commpage Target Flag PoC and crash log. For TCC database modification, use the `tccutil flag check` and `tccutil flag reset` workflow to confirm impact.',
  'Do not publicly disclose before Apple releases an update with a security advisory or otherwise completes investigation.',
  'Do not submit reports about third-party hardware, software, or services to Apple.',
  'Do not rely on theoretical, unvalidated, incomplete, or AI-discovered claims without reproducible validation.',
  'Do not brute force Target Flags.'
] as const;

const MSRC_RULES = [
  'Verify the current Windows Insider Preview bounty scope, Rules of Engagement, safe harbor, bounty guidelines, and eligibility criteria before testing or submitting.',
  'Reproduce and validate on the latest applicable Windows Insider Preview Canary build; record the tested build and exact BuildLabEx revision string.',
  'Confirm that the affected feature is serviced and eligible under the Windows Security Servicing Criteria. An installed app, service, or open-source component is a research candidate, not an automatic bounty determination.',
  'For local Attack Scenario Awards from an eligible sandbox, demonstrate the restricted context using the Launch App Container tool with the LPAC flag and only capabilities used by eligible sandboxes.',
  'Submit privately through the MSRC Researcher Portal under Coordinated Vulnerability Disclosure.',
  'Provide clear reproduction steps, proof-of-concept code when safe, detailed technical analysis, affected assets, expected and observed behavior, security impact, prerequisites, and remediation-relevant details.',
  'Prioritize new, unique vulnerabilities with meaningful real-world customer security impact.',
  'Include enough detail for Microsoft to validate, triage, reproduce, and fix the issue quickly.',
  'Follow Microsoft Security Testing Rules of Engagement and the current Windows Insider Preview bounty program page.',
  'Do not access, modify, exfiltrate, disclose, or share customer data.',
  'Do not disrupt Microsoft services, compromise uptime, degrade availability, or harm other customers or infrastructure.',
  'If unauthorized or sensitive data is encountered, stop immediately, notify MSRC with details, delete the data, and acknowledge this in the report.',
  'Do not publicly disclose before Microsoft has had time to remediate under CVD.'
] as const;

export const RESEARCH_KITS: readonly ResearchKitDefinition[] = [{
  id: 'general',
  label: 'General',
  description: 'Build the workspace scope, resources, and rules manually.',
  supportedResearchProfileIds: ['security-research', 'mathematics']
}, {
  id: 'hackerone',
  label: 'HackerOne',
  description: 'Import a public HackerOne program scope and normalize its resources and rules.',
  supportedResearchProfileIds: ['security-research'],
  scopeLookup: 'hackerone',
  refresh: {
    sourceLabel: 'HackerOne Program',
    sourceDescription: 'The public program handle or HackerOne URL used for this workspace.',
    sourceIdentifierPlaceholder: 'program-handle',
    imports: ['resources', 'rules', 'guidance']
  }
}, {
  id: 'apple-security-bounty',
  label: 'Apple Security Bounty',
  description: 'Start with Apple Security Bounty guidance and optional Apple OSS repositories.',
  supportedResearchProfileIds: ['security-research'],
  onboardingDefaults: {
    workspaceName: 'Apple Security Bounty',
    researchSubjectName: 'Apple',
    descriptionMarkdown: 'Authorized research under the Apple Security Bounty program for eligible Apple product, platform, service, and security mechanism vulnerabilities described by Apple Security Research.',
    rules: APPLE_RULES
  },
  repositoryCatalog: {
    provider: 'github-organization',
    organization: 'apple-oss-distributions',
    resourceSource: 'apple-oss'
  },
  refresh: {
    sourceLabel: 'Repository Catalog',
    sourceDescription: 'Refreshes metadata for Apple repositories already selected as workspace resources.',
    fixedSource: 'apple-oss-distributions',
    imports: ['resources', 'rules', 'guidance']
  }
}, {
  id: 'google-oss-vrp',
  label: 'Google OSS VRP',
  description: 'Start with Google Bug Hunters OSS VRP rules and optional tiered OT0/OT1 repositories.',
  supportedResearchProfileIds: ['security-research'],
  onboardingDefaults: {
    workspaceName: 'Google OSS VRP',
    researchSubjectName: 'Google Open Source Software',
    descriptionMarkdown: 'Authorized research under the Google Open Source Software Vulnerability Reward Program for eligible Google OSS repositories, repository configuration, supply-chain compromises, third-party dependencies, and product vulnerabilities. Reconfirm the current Google Bug Hunters rules and repository tier before testing or submitting.',
    rules: GOOGLE_OSS_RULES
  },
  repositoryCatalog: {
    provider: 'bundled',
    sourceUrl: GOOGLE_OSS_TIER_LIST_URL,
    repositories: GOOGLE_OSS_REPOSITORIES,
    resourceSource: 'google-oss-vrp'
  },
  refresh: {
    sourceLabel: 'Repository Tier Catalog',
    sourceDescription: 'Refreshes selected repository tier metadata plus the Google OSS VRP rules and guidance bundled with this version of Beale.',
    fixedSource: 'google/bughunters OSS repository tiers',
    imports: ['resources', 'rules', 'guidance']
  }
}, {
  id: 'msrc',
  label: 'MSRC Windows',
  description: 'Research the Windows Insider Preview bounty with selectable Windows apps, services, sandbox contexts, and shipped source repositories.',
  supportedResearchProfileIds: ['security-research'],
  onboardingDefaults: {
    workspaceName: 'MSRC Windows',
    researchSubjectName: 'Windows Insider Preview',
    descriptionMarkdown: `Authorized research under the [Microsoft Windows Insider Preview bounty program](${MSRC_WINDOWS_PROGRAM_URL}) on the latest applicable Canary build. Select only resources present on the test guest and confirm the current program scope, Windows servicing eligibility, and exact installed versions before testing. Common Windows 11 apps and services vary by edition, image, region, and update state; a catalog entry is not proof of installation or award eligibility. Open-source repositories are source references, not proof that the guest contains the same revision.\n\nThe program currently lists four sandboxes for local Attack Scenario Awards: Microsoft Edge Chromium renderer, Windows Defender (MsMpEngCP), WinHTTP WPAD sandboxed process, and UtcDecoderHost.exe. The restricted-context proof must follow the current program's Launch App Container and LPAC requirements. Windows Sandbox is a separate optional Windows feature and is not one of those four listed sandbox contexts. Recheck the [program page](${MSRC_WINDOWS_PROGRAM_URL}) before relying on any award category.`,
    rules: MSRC_RULES
  },
  resourceCatalog: {
    sourceUrl: MSRC_WINDOWS_PROGRAM_URL,
    resourceSource: 'msrc-windows',
    resources: MSRC_WINDOWS_RESOURCES
  },
  refresh: {
    sourceLabel: 'Windows Insider Preview Program',
    sourceDescription: 'Refreshes selected resource metadata, rules, and guidance bundled with this version of Beale. Check Microsoft for current terms.',
    fixedSource: 'Microsoft Windows Insider Preview',
    imports: ['resources', 'rules', 'guidance']
  }
}, {
  id: 'meta-bug-bounty',
  label: 'Meta Bug Bounty',
  description: 'Import Meta Bug Bounty scope examples, responsible research rules, and program guidance.',
  supportedResearchProfileIds: ['security-research'],
  onboardingDefaults: {
    workspaceName: 'Meta Bug Bounty',
    researchSubjectName: 'Meta',
    descriptionMarkdown: `Authorized research under the Meta Bug Bounty program. The imported resources are examples from the published scope, not an exhaustive list or authorization for third-party systems. Check [program scope](${META_BUG_BOUNTY_SCOPE_URL}) and [responsible research terms](${META_BUG_BOUNTY_TERMS_URL}) before testing. Narrow the workspace resources to the surfaces you intend to test.\n\nThe [payout guidelines](${META_BUG_BOUNTY_PAYOUT_GUIDELINES_URL}) describe category-specific maximums and mitigating factors. Meta assesses reports and deductions case by case; these guidelines do not expand testing scope.`,
    rules: META_BUG_BOUNTY_RULES,
    assets: META_BUG_BOUNTY_SCOPE_ASSETS
  },
  refresh: {
    sourceLabel: 'Program Guidance',
    sourceDescription: 'Refreshes imported scope examples, rules, and guidance bundled with this version of Beale.',
    fixedSource: 'Meta Bug Bounty',
    imports: ['resources', 'rules', 'guidance']
  }
}] as const;

export function isResearchKitId(value: unknown): value is ResearchKitId {
  return typeof value === 'string' && RESEARCH_KIT_IDS.includes(value as ResearchKitId);
}

export function researchKitDefinition(id: ResearchKitId): ResearchKitDefinition {
  return RESEARCH_KITS.find((kit) => kit.id === id) ?? RESEARCH_KITS[0];
}

export function researchKitLabel(id: ResearchKitId): string {
  return researchKitDefinition(id).label;
}

export function researchKitsForProfile(profileId: ResearchProfileId): readonly ResearchKitDefinition[] {
  return RESEARCH_KITS.filter((kit) => kit.supportedResearchProfileIds.includes(profileId));
}

export function researchKitSupportsProfile(id: ResearchKitId, profileId: ResearchProfileId): boolean {
  return researchKitDefinition(id).supportedResearchProfileIds.includes(profileId);
}

export function researchKitResourceKey(asset: Pick<ScopeAssetInput, 'direction' | 'kind' | 'value'>): string {
  return JSON.stringify([asset.direction, asset.kind, asset.value.trim().toLowerCase()]);
}

export function selectedResearchKitCatalogAssets(
  catalog: NonNullable<ResearchKitDefinition['resourceCatalog']>,
  existing: readonly ScopeAssetInput[],
  requestedKeys?: readonly string[]
): ScopeAssetInput[] {
  const catalogByKey = new Map(catalog.resources.map((asset) => [researchKitResourceKey(asset), asset]));
  const existingByKey = new Map(existing.map((asset) => [researchKitResourceKey(asset), asset]));
  const selectedKeys = requestedKeys ?? [...existingByKey.keys()].filter((key) => catalogByKey.has(key));
  if (selectedKeys.some((key) => !catalogByKey.has(key))) throw new Error('Unknown Research Kit resource selection.');
  return [...new Set(selectedKeys)].map((key) => {
    const bundled = catalogByKey.get(key)!;
    const current = existingByKey.get(key);
    return current ? {
      ...current,
      attributes: { ...current.attributes, ...bundled.attributes }
    } : { ...bundled, attributes: { ...bundled.attributes } };
  });
}

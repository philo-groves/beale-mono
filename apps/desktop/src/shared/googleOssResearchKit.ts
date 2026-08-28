export type GoogleOssRepositoryTier = 'OT0' | 'OT1';

export interface GoogleOssRepositoryCatalogEntry {
  name: string;
  url: string;
  archived: false;
  tier: GoogleOssRepositoryTier;
}

export const GOOGLE_OSS_RULES_URL = 'https://bughunters.google.com/about/rules/open-source/google-open-source-software-vulnerability-reward-program-rules';
export const GOOGLE_OSS_TIER_LIST_URL = 'https://github.com/google/bughunters/tree/main/oss-repository-tier';

export const GOOGLE_OSS_RULES = [
  `Verify the current Google Open Source Software Vulnerability Reward Program rules and repository tiers before testing or submitting: ${GOOGLE_OSS_RULES_URL}`,
  'Limit research to the latest versions of open source software in public repositories owned by Google GitHub organizations or selected repositories on other platforms. Repository configuration such as GitHub Actions, access controls, and GitHub application configuration is also in scope.',
  'Prioritize exploitable supply-chain compromises affecting Google OSS source, build integrity, distributed artifacts, package-publishing credentials, or signing keys. Demonstrate a path that bypasses required maintainer approval; a path that triggers only after maintainer approval is treated as insider risk.',
  'For product vulnerabilities, demonstrate substantial confidentiality or integrity impact to user data in software builds using the affected Google OSS project, including a realistic attack scenario and reachable vulnerable behavior.',
  'For Google open source projects closely tied to Google Cloud or AI products, follow the Bug Hunters guidance to report through the Google Cloud VRP or AI VRP when that is the appropriate route.',
  'For memory-corruption reports in OT0 or OT1 repositories, provide exact reproduction steps using an existing OSS-Fuzz target, including exact image-build steps, or submit only after a patch has already been merged in the target repository.',
  'Product vulnerabilities and other security issues in OT2 and OT3 projects are not eligible for monetary rewards. OT2 has no published repository list and final tiering is determined by the reward panel; OT3 covers low-priority, experimental, sample, inactive, unofficial, documentation-only, and similar projects.',
  'For a vulnerable third-party dependency, report to the dependency owner first, demonstrate that the issue is triggerable in Google OSS, and wait at least 30 days after the upstream fix before sharing the issue details with Google.',
  'Do not test vulnerabilities in third-party source-control, CI/CD, package-manager, or other service platforms. Only Google\'s configuration or integration of those services is within this program.',
  'Test locally whenever possible. Do not perform denial-of-service attacks, spam, black-hat SEO, disruptive activity, high-volume automated testing, unlawful testing, or access, disruption, or compromise of data that is not your own.',
  'Do not submit downstream integration mistakes, dependency-confusion or typosquatting claims without demonstrated compromise of Google-distributed artifacts, insecure developer installation guidance, negligible-impact findings, or unvalidated theoretical claims.',
  'Submit through the Google Bug Hunters vulnerability form, select OSS VRP for Bug Location, and specify the repository URL. Include the affected recent version, buildable proof of concept, exact reproduction steps, crash dump when available, technical impact, and attack scenario.'
] as const;

const OT0_REPOSITORIES = [
  'https://fuchsia.googlesource.com/fuchsia',
  'https://github.com/GoogleContainerTools/distroless',
  'https://github.com/angular/angular',
  'https://github.com/angular/angular-cli',
  'https://github.com/angular/components',
  'https://github.com/angular/dev-infra',
  'https://github.com/bazelbuild/bazel',
  'https://github.com/bazelbuild/bazel-central-registry',
  'https://github.com/bazelbuild/continuous-integration',
  'https://github.com/flutter/flutter',
  'https://github.com/golang/crypto',
  'https://github.com/golang/go',
  'https://github.com/golang/image',
  'https://github.com/golang/net',
  'https://github.com/golang/sync',
  'https://github.com/golang/sys',
  'https://github.com/golang/text',
  'https://github.com/golang/tools',
  'https://github.com/google/gson',
  'https://github.com/google/guava',
  'https://github.com/google/gvisor',
  'https://github.com/openthread/openthread',
  'https://github.com/protocolbuffers/protobuf',
  'https://github.com/tink-crypto/tink-cc',
  'https://github.com/tink-crypto/tink-go',
  'https://github.com/tink-crypto/tink-java'
] as const;

const OT1_REPOSITORIES = [
  'https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/src/starnix/',
  'https://gerrit.googlesource.com/gerrit',
  'https://github.com/ChromeDevTools/chrome-devtools-mcp',
  'https://github.com/Polymer/polymer',
  'https://github.com/bazelbuild/remote-apis-sdks',
  'https://github.com/cdapio/cdap',
  'https://github.com/cdapio/cdap-ui',
  'https://github.com/dart-lang/build',
  'https://github.com/dart-lang/sdk',
  'https://github.com/dart-lang/test',
  'https://github.com/dart-lang/tools',
  'https://github.com/flutter/devtools',
  'https://github.com/golang/vscode-go',
  'https://github.com/google-gemini/gemini-cli',
  'https://github.com/google/XNNPACK',
  'https://github.com/google/adk-go',
  'https://github.com/google/adk-java',
  'https://github.com/google/adk-js',
  'https://github.com/google/adk-python',
  'https://github.com/google/benchmark',
  'https://github.com/google/brotli',
  'https://github.com/google/closure-compiler',
  'https://github.com/google/clusterfuzz',
  'https://github.com/google/filament',
  'https://github.com/google/flatbuffers',
  'https://github.com/google/fonts',
  'https://github.com/google/go-cloud',
  'https://github.com/google/go-github',
  'https://github.com/google/libphonenumber',
  'https://github.com/google/osv-scanner',
  'https://github.com/google/re2',
  'https://github.com/google/shaderc',
  'https://github.com/google/site-kit-wp',
  'https://github.com/google/zerocopy',
  'https://github.com/googlemaps/android-maps-utils',
  'https://github.com/jax-ml/jax',
  'https://github.com/keras-team/keras',
  'https://github.com/keras-team/tf-keras',
  'https://github.com/openxla/xla',
  'https://github.com/project-oak/oak',
  'https://github.com/shaka-project/shaka-player',
  'https://github.com/tensorflow/models',
  'https://github.com/tensorflow/serving',
  'https://github.com/tensorflow/tensorflow',
  'https://github.com/tensorflow/tfjs',
  'https://github.com/tensorflow/tflite-micro'
] as const;

export const GOOGLE_OSS_REPOSITORIES: readonly GoogleOssRepositoryCatalogEntry[] = [
  ...OT0_REPOSITORIES.map((url) => googleOssRepository(url, 'OT0')),
  ...OT1_REPOSITORIES.map((url) => googleOssRepository(url, 'OT1'))
];

function googleOssRepository(url: string, tier: GoogleOssRepositoryTier): GoogleOssRepositoryCatalogEntry {
  return { name: googleOssRepositoryName(url), url, archived: false, tier };
}

function googleOssRepositoryName(url: string): string {
  const parsed = new URL(url);
  const segments = parsed.pathname.split('/').filter(Boolean);
  if (parsed.hostname === 'github.com') return segments.slice(0, 2).join('/');
  if (url.includes('/src/starnix/')) return 'fuchsia/starnix';
  return `${parsed.hostname}/${segments[0] ?? ''}`.replace(/\/$/u, '');
}

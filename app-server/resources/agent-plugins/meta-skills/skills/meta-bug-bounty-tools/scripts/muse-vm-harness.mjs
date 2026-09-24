#!/usr/bin/env node
import { createHash, randomBytes } from 'node:crypto';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const source = 'https://research.meta.ai/blog/security-and-safety-for-ai-agents-our-approach-with-muse';
const probePath = fileURLToPath(new URL('./muse-runtime-probe.sh', import.meta.url));
const header = 'BEALE_MUSE_BOUNDARY_V1';
const footer = 'END_BEALE_MUSE_BOUNDARY_V1';
const fields = ['uid', 'uid_map', 'cap_eff', 'cap_bnd'];

function usage() {
  throw new Error('Usage: muse-vm-harness.mjs init --out DIRECTORY | assess --run DIRECTORY --input CAPTURE_FILE');
}

function options(args) {
  if (args.length % 2 !== 0) usage();
  const parsed = new Map();
  for (let i = 0; i < args.length; i += 2) {
    if (!args[i].startsWith('--') || !args[i + 1] || parsed.has(args[i])) usage();
    parsed.set(args[i], args[i + 1]);
  }
  return parsed;
}

function assertOptions(args, allowed, required) {
  for (const key of args.keys()) if (!allowed.includes(key)) usage();
  for (const key of required) if (!args.has(key)) usage();
}

function parseCapture(raw, runId) {
  const lines = raw.split(/\r?\n/);
  const start = lines.findIndex((line) => line === `${header}\t${runId}`);
  if (start < 0) throw new Error('Capture has no matching run header.');
  const end = lines.findIndex((line, index) => index > start && line === `${footer}\t${runId}`);
  if (end < 0) throw new Error('Capture has no matching run footer.');
  const result = {};
  for (const line of lines.slice(start + 1, end)) {
    const match = /^([a-z_]+)\t([^\r\n]+)$/.exec(line);
    if (!match || !fields.includes(match[1]) || Object.hasOwn(result, match[1])) {
      throw new Error('Capture contains a malformed or duplicate field.');
    }
    result[match[1]] = match[2];
  }
  if (fields.some((field) => !Object.hasOwn(result, field))) {
    throw new Error('Capture is missing a required field.');
  }
  return result;
}

function capabilities(value) {
  if (!/^[0-9a-fA-F]+$/.test(value)) return null;
  const bits = BigInt(`0x${value}`);
  return {
    netAdmin: (bits & (1n << 12n)) !== 0n,
    sysPtrace: (bits & (1n << 19n)) !== 0n
  };
}

function assess(observed) {
  const match = /^(\d+) (\d+) (\d+)$/.exec(observed.uid_map);
  const uidMap = match
    ? { inside: Number(match[1]), outside: Number(match[2]), count: Number(match[3]) }
    : null;
  const effective = capabilities(observed.cap_eff);
  const bounding = capabilities(observed.cap_bnd);
  const checks = {
    rootMappedAwayFromHostRoot: uidMap && uidMap.inside === 0 && uidMap.count > 0
      ? uidMap.outside !== 0 : null,
    effectiveNetAdminAbsent: effective ? !effective.netAdmin : null,
    effectiveSysPtraceAbsent: effective ? !effective.sysPtrace : null,
    boundingNetAdminAbsent: bounding ? !bounding.netAdmin : null,
    boundingSysPtraceAbsent: bounding ? !bounding.sysPtrace : null
  };
  return {
    checks,
    result: Object.values(checks).includes(false) ? 'deviation-needs-review'
      : Object.values(checks).includes(null) ? 'incomplete' : 'baseline-consistent'
  };
}

async function init(directory) {
  const path = resolve(directory);
  const runId = randomBytes(16).toString('hex');
  const manifest = {
    schemaVersion: 1,
    runId,
    createdAt: new Date().toISOString(),
    source,
    probe: 'muse-runtime-probe.sh',
    scope: 'Read-only checks in the researcher-controlled Muse runtime cell',
    note: 'This checks published isolation properties. It does not prove a VM escape or test host-side services.'
  };
  await mkdir(path, { recursive: true });
  await writeFile(join(path, 'run.json'), `${JSON.stringify(manifest, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
  await writeFile(join(path, 'muse-runtime-probe.sh'), await readFile(probePath), { flag: 'wx', mode: 0o700 });
  process.stdout.write(`${path}\nRun ID: ${runId}\n`);
}

async function analyze(directory, inputPath) {
  const path = resolve(directory);
  const manifest = JSON.parse(await readFile(join(path, 'run.json'), 'utf8'));
  if (manifest.schemaVersion !== 1 || !/^[a-f0-9]{32}$/.test(manifest.runId)) {
    throw new Error('Invalid run manifest.');
  }
  const raw = await readFile(resolve(inputPath), 'utf8');
  if (raw.length > 32768) throw new Error('Capture exceeds 32 KiB.');
  const observed = parseCapture(raw, manifest.runId);
  const evaluation = assess(observed);
  const report = {
    schemaVersion: 1,
    runId: manifest.runId,
    source,
    captureSha256: createHash('sha256').update(raw).digest('hex'),
    observed,
    ...evaluation,
    interpretation: 'A deviation is a lead to verify, not evidence of host access or a confirmed escape.'
  };
  const outputPath = join(path, 'assessment.json');
  await writeFile(outputPath, `${JSON.stringify(report, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
  process.stdout.write(`${outputPath}\n${evaluation.result}\n`);
}

try {
  const [command, ...rest] = process.argv.slice(2);
  const args = options(rest);
  if (command === 'init') {
    assertOptions(args, ['--out'], ['--out']);
    await init(args.get('--out'));
  } else if (command === 'assess') {
    assertOptions(args, ['--run', '--input'], ['--run', '--input']);
    await analyze(args.get('--run'), args.get('--input'));
  } else {
    usage();
  }
} catch (error) {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
  process.exitCode = 1;
}

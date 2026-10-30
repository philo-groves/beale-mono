import { homedir } from 'node:os';
import { join } from 'node:path';
import type { FleetBackend } from '@beale/app-server-runtime/protocol';

export function sshConnectionArgs(host: string, user: string, identity: string | null, knownHosts: string | null,
  backend: FleetBackend, hostKeyAlias: string | null = null): string[] {
  if (!/^[A-Za-z_][A-Za-z0-9_.-]{0,63}$/u.test(user) || !/^[A-Za-z0-9][A-Za-z0-9.:-]*$/u.test(host)) {
    throw new Error('Invalid Fleet SSH destination.');
  }
  if (hostKeyAlias && !/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/u.test(hostKeyAlias)) {
    throw new Error('Invalid Fleet SSH host-key alias.');
  }
  const tartKnownHosts = join(homedir(), '.ssh', 'tart_known_hosts');
  const knownHostsFile = knownHosts ? expandHome(knownHosts) : backend === 'tart' ? tartKnownHosts : null;
  const hostKeyChecking = backend === 'tart' && knownHostsFile ? 'accept-new' : 'yes';
  return [
    '-o', 'BatchMode=yes', '-o', `StrictHostKeyChecking=${hostKeyChecking}`, '-o', 'ConnectTimeout=10',
    '-o', 'IdentitiesOnly=yes', '-o', 'PasswordAuthentication=no', '-o', 'KbdInteractiveAuthentication=no',
    ...(hostKeyAlias ? ['-o', `HostKeyAlias=${hostKeyAlias}`, '-o', 'CheckHostIP=no'] : []),
    ...(knownHostsFile ? ['-o', `UserKnownHostsFile=${knownHostsFile}`] : []),
    ...(identity ? ['-i', expandHome(identity)] : []), `${user}@${host}`,
  ];
}

function expandHome(path: string): string {
  return path === '~' ? homedir() : path.startsWith('~/') ? join(homedir(), path.slice(2)) : path;
}

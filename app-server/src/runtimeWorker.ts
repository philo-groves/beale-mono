import { parentPort, workerData } from 'node:worker_threads';
import { PassThrough } from 'node:stream';
import { installUndiciTypeOfServiceCompatibility } from '@beale/app-server-runtime/node-network-compatibility';
import {
  installResearchDatabaseFactory,
  type ResearchLiveEventSink
} from '@beale/app-server-runtime/runtime-services';
import type { AppServerRuntimeTransport } from '@beale/app-server-runtime/runtime';
import { createWorkerResearchDatabaseFactory } from './workerDatabaseClient.js';

installUndiciTypeOfServiceCompatibility();

interface RuntimeWorkerData {
  args: string[];
  env: Record<string, string>;
}

type HostMessage =
  | { type: 'control'; control: Record<string, unknown> }
  | { type: 'stop' };

const port = parentPort;
if (!port) throw new Error('The app-server runtime worker requires a parent port.');

installResearchDatabaseFactory(createWorkerResearchDatabaseFactory((message) => port.postMessage(message)));

const data = workerData as RuntimeWorkerData;
for (const [name, value] of Object.entries(data.env)) process.env[name] = value;

const controlInput = new PassThrough();
const eventSink: ResearchLiveEventSink = async (event) => {
  port.postMessage({ type: 'event', event });
};
const transport: AppServerRuntimeTransport = {
  controlInput,
  eventSink,
  waitForClient: async () => undefined,
  close: async () => { controlInput.end(); }
};

port.on('message', (message: HostMessage) => {
  if (message.type === 'control') {
    controlInput.write(`${JSON.stringify(message.control)}\n`, 'utf8');
  } else if (message.type === 'stop') {
    controlInput.write(`${JSON.stringify({ schemaVersion: 1, type: 'stop', requestId: 'app_server_stop' })}\n`, 'utf8');
  }
});

try {
  const { main } = await import('@beale/app-server-runtime/runtime');
  await main(data.args, { transport });
  port.postMessage({ type: 'complete', exitCode: process.exitCode ?? 0 });
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);
  port.postMessage({
    type: 'failed',
    error: message
  });
  process.stderr.write(`${message}\n`);
}
await new Promise<void>((resolve) => setImmediate(resolve));
port.close();

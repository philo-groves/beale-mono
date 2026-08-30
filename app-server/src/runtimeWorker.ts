import { parentPort, workerData } from 'node:worker_threads';
import { PassThrough } from 'node:stream';
import { installUndiciTypeOfServiceCompatibility } from 'honeycrisp/node-network-compatibility';
import type { ResearchLiveEventSink } from 'honeycrisp/runtime-services';
import type { HoneycrispRuntimeTransport } from 'honeycrisp/runtime';

installUndiciTypeOfServiceCompatibility();

interface RuntimeWorkerData {
  args: string[];
  env: Record<string, string>;
}

type HostMessage =
  | { type: 'control'; control: Record<string, unknown> }
  | { type: 'stop' };

const port = parentPort;
if (!port) throw new Error('The Honeycrisp runtime worker requires a parent port.');

const data = workerData as RuntimeWorkerData;
for (const [name, value] of Object.entries(data.env)) process.env[name] = value;

const controlInput = new PassThrough();
const eventSink: ResearchLiveEventSink = async (event) => {
  port.postMessage({ type: 'event', event });
};
const transport: HoneycrispRuntimeTransport = {
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
  const { main } = await import('honeycrisp/runtime');
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

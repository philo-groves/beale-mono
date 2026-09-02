/**
 * Beale's single adapter over the versioned app-server protocol DTOs.
 *
 * Shared DTOs, constants, and decoders come from the `@beale/app-server-runtime/protocol`
 * package so the app-server runtime and this host cannot drift. Only genuinely
 * Beale-side policy lives here: the required-capability gate, the hardened
 * bootstrap parser, and the pinned `beale` client identity.
 */
import {
  APP_SERVER_CONTRACT_VERSION,
  APP_SERVER_PROTOCOL_BOOTSTRAP_PREFIX,
  APP_SERVER_PROTOCOL_CAPABILITIES,
  APP_SERVER_PROTOCOL_NAME,
  APP_SERVER_PROTOCOL_VERSION,
  APP_SERVER_PROTOCOL_WEBSOCKET_PATH,
  decodeAppServerProtocolEnvelope as decodeAppServerProtocolEnvelopeValue,
  type AppServerClientHello,
  type AppServerProtocolDescriptor,
  type AppServerProtocolEnvelope,
  type AppServerSessionControl,
  type AppServerTransportBootstrap
} from '@beale/app-server-runtime/protocol';

export {
  APP_SERVER_CONTRACT_VERSION,
  APP_SERVER_PROTOCOL_BOOTSTRAP_PREFIX,
  APP_SERVER_PROTOCOL_MAX_REQUEST_ID_LENGTH,
  APP_SERVER_PROTOCOL_NAME,
  APP_SERVER_PROTOCOL_VERSION,
  APP_SERVER_PROTOCOL_WEBSOCKET_PATH,
  APP_SERVER_PROTOCOL_WEBSOCKET_CAPABILITIES,
  decodeAppServerServerMessage,
  type AppServerClientHello,
  type AppServerProtocolDescriptor,
  type AppServerProtocolErrorDetail,
  type AppServerProtocolEnvelope,
  type AppServerProtocolFailure,
  type AppServerProtocolSuccess,
  type AppServerServerHello,
  type AppServerServerMessage,
  type AppServerSessionControl,
  type AppServerTransportBootstrap,
  type AppServerWebSocketProtocolError
} from '@beale/app-server-runtime/protocol';

/** Capabilities Beale requires before it will talk to an app-server runtime. */
export const APP_SERVER_REQUIRED_CAPABILITIES = APP_SERVER_PROTOCOL_CAPABILITIES;

export function appServerClientHello(sessionId: string, clientVersion: string): AppServerClientHello {
  return {
    protocolVersion: APP_SERVER_PROTOCOL_VERSION,
    type: 'client.hello',
    sessionId,
    client: { name: 'beale', version: clientVersion }
  };
}

export function appServerSessionControl(
  sessionId: string,
  control: Record<string, unknown> & { requestId: string }
): AppServerSessionControl {
  return {
    protocolVersion: APP_SERVER_PROTOCOL_VERSION,
    type: 'session.control',
    sessionId,
    requestId: control.requestId,
    control
  };
}

export function decodeAppServerProtocolEnvelope<T>(json: string): AppServerProtocolEnvelope<T> {
  const parsed: unknown = JSON.parse(json);
  return decodeAppServerProtocolEnvelopeValue(parsed) as unknown as AppServerProtocolEnvelope<T>;
}

export function decodeAppServerProtocolDescriptor(value: unknown): AppServerProtocolDescriptor {
  const capabilities = isRecord(value) && Array.isArray(value.capabilities) ? value.capabilities : null;
  if (!isRecord(value)
    || value.protocol !== APP_SERVER_PROTOCOL_NAME
    || value.protocolVersion !== APP_SERVER_PROTOCOL_VERSION
    || value.contractVersion !== APP_SERVER_CONTRACT_VERSION
    || !Array.isArray(value.operations) || !value.operations.every(nonEmptyString)
    || !isRecord(value.runtime) || value.runtime.name !== APP_SERVER_PROTOCOL_NAME
    || !nonEmptyString(value.runtime.version) || !nonEmptyString(value.runtime.buildId) || !nonEmptyString(value.runtime.nodeVersion)
    || !validSchemaDescriptor(value.schemas)
    || !capabilities || !capabilities.every(nonEmptyString)
    || !APP_SERVER_REQUIRED_CAPABILITIES.every((capability) => capabilities.includes(capability))
    || !isRecord(value.transports)) {
    throw new Error(`app-server runtime is incompatible with Beale contract v${APP_SERVER_CONTRACT_VERSION}. Rebuild or restart the Beale app-server.`);
  }
  return value as unknown as AppServerProtocolDescriptor;
}

export function parseAppServerTransportBootstrap(
  line: string,
  expectedSessionId: string
): AppServerTransportBootstrap | null {
  if (!line.startsWith(APP_SERVER_PROTOCOL_BOOTSTRAP_PREFIX)) return null;
  try {
    const value = JSON.parse(line.slice(APP_SERVER_PROTOCOL_BOOTSTRAP_PREFIX.length)) as unknown;
    if (!isRecord(value) || value.protocolVersion !== APP_SERVER_PROTOCOL_VERSION
      || value.transport !== 'websocket' || value.sessionId !== expectedSessionId
      || !nonEmptyString(value.url)) return null;
    const url = new URL(value.url);
    if (url.protocol !== 'ws:' || url.hostname !== '127.0.0.1'
      || url.pathname !== APP_SERVER_PROTOCOL_WEBSOCKET_PATH || url.username || url.password) return null;
    return {
      protocolVersion: APP_SERVER_PROTOCOL_VERSION,
      transport: 'websocket',
      url: url.toString(),
      sessionId: value.sessionId
    };
  } catch {
    return null;
  }
}

function validSchemaDescriptor(value: unknown): boolean {
  return isRecord(value) && value.protocol === 1 && value.session === 1
    && value.memorySummary === 12 && value.finding === 5 && value.campaignGraph === 4;
}

function nonEmptyString(value: unknown): value is string {
  return typeof value === 'string' && Boolean(value.trim());
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

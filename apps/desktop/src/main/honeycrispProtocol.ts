/**
 * Beale's single adapter over the versioned Honeycrisp protocol DTOs.
 *
 * Shared DTOs, constants, and decoders come from the `honeycrisp/protocol`
 * package so the app-server runtime and this host cannot drift. Only genuinely
 * Beale-side policy lives here: the required-capability gate, the hardened
 * bootstrap parser, and the pinned `beale` client identity.
 */
import {
  HONEYCRISP_CONTRACT_VERSION,
  HONEYCRISP_PROTOCOL_BOOTSTRAP_PREFIX,
  HONEYCRISP_PROTOCOL_CAPABILITIES,
  HONEYCRISP_PROTOCOL_NAME,
  HONEYCRISP_PROTOCOL_VERSION,
  HONEYCRISP_PROTOCOL_WEBSOCKET_PATH,
  decodeHoneycrispProtocolEnvelope as decodeHoneycrispProtocolEnvelopeValue,
  type HoneycrispClientHello,
  type HoneycrispProtocolDescriptor,
  type HoneycrispProtocolEnvelope,
  type HoneycrispSessionControl,
  type HoneycrispTransportBootstrap
} from 'honeycrisp/protocol';

export {
  HONEYCRISP_CONTRACT_VERSION,
  HONEYCRISP_PROTOCOL_BOOTSTRAP_PREFIX,
  HONEYCRISP_PROTOCOL_MAX_REQUEST_ID_LENGTH,
  HONEYCRISP_PROTOCOL_NAME,
  HONEYCRISP_PROTOCOL_VERSION,
  HONEYCRISP_PROTOCOL_WEBSOCKET_PATH,
  HONEYCRISP_PROTOCOL_WEBSOCKET_CAPABILITIES,
  decodeHoneycrispServerMessage,
  type HoneycrispClientHello,
  type HoneycrispProtocolDescriptor,
  type HoneycrispProtocolErrorDetail,
  type HoneycrispProtocolEnvelope,
  type HoneycrispProtocolFailure,
  type HoneycrispProtocolSuccess,
  type HoneycrispServerHello,
  type HoneycrispServerMessage,
  type HoneycrispSessionControl,
  type HoneycrispTransportBootstrap,
  type HoneycrispWebSocketProtocolError
} from 'honeycrisp/protocol';

/** Capabilities Beale requires before it will talk to a Honeycrisp runtime. */
export const HONEYCRISP_REQUIRED_CAPABILITIES = HONEYCRISP_PROTOCOL_CAPABILITIES;

export function honeycrispClientHello(sessionId: string, clientVersion: string): HoneycrispClientHello {
  return {
    protocolVersion: HONEYCRISP_PROTOCOL_VERSION,
    type: 'client.hello',
    sessionId,
    client: { name: 'beale', version: clientVersion }
  };
}

export function honeycrispSessionControl(
  sessionId: string,
  control: Record<string, unknown> & { requestId: string }
): HoneycrispSessionControl {
  return {
    protocolVersion: HONEYCRISP_PROTOCOL_VERSION,
    type: 'session.control',
    sessionId,
    requestId: control.requestId,
    control
  };
}

export function decodeHoneycrispProtocolEnvelope<T>(json: string): HoneycrispProtocolEnvelope<T> {
  const parsed: unknown = JSON.parse(json);
  return decodeHoneycrispProtocolEnvelopeValue(parsed) as unknown as HoneycrispProtocolEnvelope<T>;
}

export function decodeHoneycrispProtocolDescriptor(value: unknown): HoneycrispProtocolDescriptor {
  const capabilities = isRecord(value) && Array.isArray(value.capabilities) ? value.capabilities : null;
  if (!isRecord(value)
    || value.protocol !== HONEYCRISP_PROTOCOL_NAME
    || value.protocolVersion !== HONEYCRISP_PROTOCOL_VERSION
    || value.contractVersion !== HONEYCRISP_CONTRACT_VERSION
    || !Array.isArray(value.operations) || !value.operations.every(nonEmptyString)
    || !isRecord(value.runtime) || value.runtime.name !== HONEYCRISP_PROTOCOL_NAME
    || !nonEmptyString(value.runtime.version) || !nonEmptyString(value.runtime.buildId) || !nonEmptyString(value.runtime.nodeVersion)
    || !validSchemaDescriptor(value.schemas)
    || !capabilities || !capabilities.every(nonEmptyString)
    || !HONEYCRISP_REQUIRED_CAPABILITIES.every((capability) => capabilities.includes(capability))
    || !isRecord(value.transports)) {
    throw new Error(`Honeycrisp runtime is incompatible with Beale contract v${HONEYCRISP_CONTRACT_VERSION}. Rebuild or restart the Beale app-server.`);
  }
  return value as unknown as HoneycrispProtocolDescriptor;
}

export function parseHoneycrispTransportBootstrap(
  line: string,
  expectedSessionId: string
): HoneycrispTransportBootstrap | null {
  if (!line.startsWith(HONEYCRISP_PROTOCOL_BOOTSTRAP_PREFIX)) return null;
  try {
    const value = JSON.parse(line.slice(HONEYCRISP_PROTOCOL_BOOTSTRAP_PREFIX.length)) as unknown;
    if (!isRecord(value) || value.protocolVersion !== HONEYCRISP_PROTOCOL_VERSION
      || value.transport !== 'websocket' || value.sessionId !== expectedSessionId
      || !nonEmptyString(value.url)) return null;
    const url = new URL(value.url);
    if (url.protocol !== 'ws:' || url.hostname !== '127.0.0.1'
      || url.pathname !== HONEYCRISP_PROTOCOL_WEBSOCKET_PATH || url.username || url.password) return null;
    return {
      protocolVersion: HONEYCRISP_PROTOCOL_VERSION,
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
    && value.memorySummary === 11 && value.finding === 4 && value.campaignGraph === 4;
}

function nonEmptyString(value: unknown): value is string {
  return typeof value === 'string' && Boolean(value.trim());
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

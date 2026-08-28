const PAIRING_SCHEME = 'beale:';
const PAIRING_HOST = 'connect';
const PAIRING_VERSION = '1';
const MAX_OPERATOR_TOKEN_CHARS = 512;

export function createAppServerPairingPayload(serverUrl: string, operatorToken: string): string {
  const endpoint = new URL(serverUrl);
  if ((endpoint.protocol !== 'http:' && endpoint.protocol !== 'https:')
    || endpoint.username
    || endpoint.password
    || (endpoint.pathname !== '/' && endpoint.pathname !== '')
    || endpoint.search
    || endpoint.hash) {
    throw new Error('The app-server pairing endpoint must be an HTTP or HTTPS origin.');
  }
  const token = operatorToken.trim();
  if (!token || token.length > MAX_OPERATOR_TOKEN_CHARS || /[\u0000-\u001f\u007f]/u.test(token)) {
    throw new Error('The app-server operator token cannot be encoded for pairing.');
  }

  const payload = new URL(`${PAIRING_SCHEME}//${PAIRING_HOST}`);
  payload.searchParams.set('v', PAIRING_VERSION);
  payload.searchParams.set('url', endpoint.origin);
  payload.searchParams.set('token', token);
  return payload.toString();
}

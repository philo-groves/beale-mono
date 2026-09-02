import { Socket } from 'node:net';

interface TypeOfServiceSocket {
  setTypeOfService?: (this: TypeOfServiceSocket, typeOfService: number) => TypeOfServiceSocket;
}

const installedPrototypes = new WeakSet<object>();

/**
 * Protect Node's bundled Undici from a macOS socket-state failure in its
 * best-effort IP quality-of-service hint. Remove this after every supported
 * host runtime contains nodejs/undici#5547.
 */
export function installUndiciTypeOfServiceCompatibility(
  socketPrototype: TypeOfServiceSocket = Socket.prototype as TypeOfServiceSocket
): boolean {
  if (installedPrototypes.has(socketPrototype)) return false;
  const setTypeOfService = socketPrototype.setTypeOfService;
  if (typeof setTypeOfService !== 'function') return false;

  socketPrototype.setTypeOfService = function guardedSetTypeOfService(typeOfService): TypeOfServiceSocket {
    try {
      return setTypeOfService.call(this, typeOfService);
    } catch (error) {
      if (!isTypeOfServiceInvalidArgument(error)) throw error;
      return this;
    }
  };
  installedPrototypes.add(socketPrototype);
  return true;
}

export function isTypeOfServiceInvalidArgument(error: unknown): boolean {
  if (!(error instanceof Error)) return false;
  const systemError = error as NodeJS.ErrnoException;
  if (systemError.code !== 'EINVAL') return false;
  return systemError.syscall === 'setTypeOfService'
    || error.message.includes('setTypeOfService');
}

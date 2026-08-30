import assert from 'node:assert/strict';
import test from 'node:test';
import {
  installUndiciTypeOfServiceCompatibility,
  isTypeOfServiceInvalidArgument
} from '../packages/honeycrisp-host/dist/node-network-compatibility.js';

function typeOfServiceError(code = 'EINVAL', syscall = 'setTypeOfService') {
  return Object.assign(new Error(`${syscall} ${code}`), { code, syscall });
}

test('recognizes only setTypeOfService EINVAL failures', () => {
  assert.equal(isTypeOfServiceInvalidArgument(typeOfServiceError()), true);
  assert.equal(isTypeOfServiceInvalidArgument(typeOfServiceError('EACCES')), false);
  assert.equal(isTypeOfServiceInvalidArgument(typeOfServiceError('EINVAL', 'connect')), false);
  assert.equal(isTypeOfServiceInvalidArgument(new Error('setTypeOfService EINVAL')), false);
});

test('contains Undici setTypeOfService EINVAL without hiding other socket failures', () => {
  const socketPrototype = {
    setTypeOfService(typeOfService) {
      if (typeOfService === 0) throw typeOfServiceError();
      if (typeOfService === 1) throw typeOfServiceError('EACCES');
      return this;
    }
  };
  const socket = Object.create(socketPrototype);

  assert.equal(installUndiciTypeOfServiceCompatibility(socketPrototype), true);
  assert.equal(socket.setTypeOfService(0), socket);
  assert.throws(() => socket.setTypeOfService(1), { code: 'EACCES' });
  assert.equal(socket.setTypeOfService(2), socket);
  assert.equal(installUndiciTypeOfServiceCompatibility(socketPrototype), false);
});

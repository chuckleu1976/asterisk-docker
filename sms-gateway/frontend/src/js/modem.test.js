import assert from 'node:assert/strict';
import test from 'node:test';
import { getNetworkStatus } from './modem.js';

test('IMS registration is displayed for string and numeric status codes', () => {
    for (const status of [1, '1', ' 1 ']) {
        assert.equal(getNetworkStatus({ status }).key, 'net_home');
    }
    for (const status of [0, '0', ' 0 ']) {
        assert.equal(getNetworkStatus({ status }).key, 'net_not_registered');
    }
});

test('missing IMS status is explicitly unknown, never blank or assumed unregistered', () => {
    for (const registration of [null, undefined, {}, { status: null }, { status: '' }, { status: ' ' }]) {
        assert.equal(getNetworkStatus(registration).key, 'net_unknown');
    }
});

test('existing network status labels and colors are preserved', () => {
    for (const [status, key, cls] of [
        ['2', 'net_searching', 'text-yellow-400'],
        ['3', 'net_denied', 'text-red-500'],
        ['4', 'net_unknown', 'text-gray-400'],
        ['5', 'net_roaming', 'text-blue-400'],
    ]) {
        assert.deepEqual(getNetworkStatus({ status }), { key, cls });
    }
});

test('unrecognized network status remains visible as a diagnostic code', () => {
    assert.deepEqual(getNetworkStatus({ status: '9' }), {
        key: 'net_code', codeN: '9', cls: 'text-gray-400',
    });
});

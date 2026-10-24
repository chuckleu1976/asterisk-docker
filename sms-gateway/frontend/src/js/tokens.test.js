import assert from 'node:assert/strict';
import test from 'node:test';
import { getTokenStatus, tokenRequest } from './tokens.js';

const currentTime = Date.parse('2026-10-24T02:00:00Z');
const rental = {
    starts_at: '2026-10-23T00:00:00Z',
    expires_at: '2026-11-28T00:00:00Z',
    permissions: ['sms:read', 'sms:send'],
};
const token = { active: true, expires_at: null, permissions: ['sms:read'] };

test('new token requests have no independent expiry', () => {
    assert.deepEqual(tokenRequest('Automation', ['sms:read']), {
        name: 'Automation', permissions: ['sms:read'],
    });
    assert.equal('expires_at' in tokenRequest('Automation', ['sms:read']), false);
});

test('rental-bound token follows expiry and resumes after renewal', () => {
    assert.equal(getTokenStatus(token, [rental], currentTime), 'tokens_active');
    assert.equal(getTokenStatus(token, [rental], Date.parse(rental.expires_at)), 'tokens_no_access');
    assert.equal(getTokenStatus(token, [], currentTime), 'tokens_no_access');
    assert.equal(getTokenStatus(token, [{ ...rental, expires_at: '2027-01-01T00:00:00Z' }],
        Date.parse('2026-12-01T00:00:00Z')), 'tokens_active');
});

test('one expired rental does not hide another valid rental', () => {
    const expired = { ...rental, expires_at: '2026-10-01T00:00:00Z' };
    assert.equal(getTokenStatus(token, [expired, rental], currentTime), 'tokens_active');
});

test('revoked, future, invalid and out-of-scope rentals do not grant access', () => {
    for (const candidate of [
        { ...rental, active: false },
        { ...rental, valid: false },
        { ...rental, starts_at: '2026-12-01T00:00:00Z' },
        { ...rental, permissions: ['esim:read'] },
        { ...rental, expires_at: 'invalid' },
    ]) {
        assert.equal(getTokenStatus(token, [candidate], currentTime), 'tokens_no_access');
    }
});

test('revoked tokens do not regain access when rentals are renewed', () => {
    assert.equal(getTokenStatus({ ...token, active: false }, [rental], currentTime), 'tokens_revoked');
});

test('legacy credential deadlines remain distinct from rental access', () => {
    assert.equal(getTokenStatus({ ...token, expires_at: '2026-10-01T00:00:00Z' }, [rental], currentTime),
        'tokens_expired');
    assert.equal(getTokenStatus({ ...token, expires_at: '2026-11-01T00:00:00Z' }, [rental], currentTime),
        'tokens_active');
});

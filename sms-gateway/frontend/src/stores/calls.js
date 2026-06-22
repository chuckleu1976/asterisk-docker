import { get, writable } from 'svelte/store';
import { apiClient } from '../js/api.js';
import { EventSourcePolyfill } from 'event-source-polyfill';

/** @typedef {{ id: string, sim_id: string, phone: string|null, direction: string, status: string, started_at: string, ended_at: string|null }} Call */
/** @typedef {{ event_type: string, sim_id: string, call_id: string, phone: string|null, direction: string }} CallEvent */

/** Currently ringing inbound call (null = no incoming call) */
export const incomingCall = writable(/** @type {CallEvent|null} */ (null));

/** Currently active call (answered or outbound in progress) */
export const activeCall = writable(/** @type {CallEvent|null} */ (null));

/** Recent call log entries */
export const callLog = writable(/** @type {Call[]} */ ([]));

export const callSseConnected = writable(false);

let eventSource = null;
let reconnectTimeout = null;
const RECONNECT_DELAY = 5000;

// ── Call status polling (fallback for missed SSE events) ──────────────────────
let callStatusPoller = null;

function startCallStatusPolling(callId) {
    stopCallStatusPolling();
    let pollCount = 0;
    callStatusPoller = setInterval(async () => {
        // Stop after 120 seconds max (safety net)
        if (++pollCount > 60) { activeCall.set(null); stopCallStatusPolling(); return; }
        try {
            const res = await apiClient.getCallLog(null, 20);
            const calls = res.data?.data ?? [];
            const call = calls.find(c => c.id === callId);
            if (call && call.status !== 'ringing') {
                // Call is no longer ringing — update banner based on actual status
                if (call.status === 'active') {
                    activeCall.update(v => v ? { ...v, event_type: 'call_answered' } : null);
                } else {
                    // ended or missed — clear banner
                    activeCall.set(null);
                    callLog.set(calls);
                    stopCallStatusPolling();
                }
            }
        } catch (_) { /* ignore poll errors */ }
    }, 2000);
}

function stopCallStatusPolling() {
    if (callStatusPoller) { clearInterval(callStatusPoller); callStatusPoller = null; }
}
// ─────────────────────────────────────────────────────────────────────────────

const getAuthHeader = () => {
    const auth = sessionStorage.getItem('auth');
    if (auth) {
        const { token } = JSON.parse(auth);
        return { Authorization: `Basic ${token}` };
    }
    return {};
};

const handleCallEvent = (/** @type {CallEvent} */ event) => {
    switch (event.event_type) {
        case 'incoming_call':
            incomingCall.set(event);
            break;
        case 'outbound_call':
        case 'outbound_call_started':
            activeCall.set(event);
            break;
        case 'call_answered':
            // Move from incoming to active
            incomingCall.set(null);
            activeCall.set(event);
            break;
        case 'call_ended':
            incomingCall.set(null);
            activeCall.set(null);
            stopCallStatusPolling();
            // Refresh the top of the call log
            callActions.refreshLog();
            break;
    }
};

export const connectCallSSE = () => {
    if (eventSource) eventSource.close();

    const authHeader = getAuthHeader();
    if (!authHeader.Authorization) return;

    eventSource = new EventSourcePolyfill('/api/calls/sse', {
        headers: authHeader,
        heartbeatTimeout: 45000,
    });

    eventSource.onopen = () => {
        callSseConnected.set(true);
        if (reconnectTimeout) {
            clearTimeout(reconnectTimeout);
            reconnectTimeout = null;
        }
    };

    eventSource.onerror = () => {
        callSseConnected.set(false);
        if (eventSource) eventSource.close();
        if (!reconnectTimeout) {
            reconnectTimeout = setTimeout(() => {
                reconnectTimeout = null;
                connectCallSSE();
            }, RECONNECT_DELAY);
        }
    };

    eventSource.addEventListener('call_event', (e) => {
        try {
            handleCallEvent(JSON.parse(e.data));
        } catch (err) {
            console.error('Failed to parse call_event:', err);
        }
    });
};

export const disconnectCallSSE = () => {
    if (eventSource) {
        eventSource.close();
        eventSource = null;
    }
    if (reconnectTimeout) {
        clearTimeout(reconnectTimeout);
        reconnectTimeout = null;
    }
    callSseConnected.set(false);
};

export const callActions = {
    async make(simId, phone) {
        // Set banner BEFORE the await so SSE call_ended can clear it correctly.
        // If we set it AFTER, a fast rejection sends call_ended before the await
        // resolves, clears activeCall, then the post-await set re-shows the banner forever.
        activeCall.set({ event_type: 'outbound_call', sim_id: simId, call_id: '', phone, direction: 'outbound' });
        try {
            const res = await apiClient.makeCall(simId, phone);
            // Patch in the real call_id, but only if the call hasn't already been ended by SSE.
            const callId = res?.data?.call_id ?? res?.call_id ?? '';
            if (callId) {
                activeCall.update(v => v ? { ...v, call_id: callId } : null);
                // Always start polling — SSE may have already cleared activeCall but
                // polling ensures the banner reflects true call state regardless.
                startCallStatusPolling(callId);
            }
        } catch (e) {
            activeCall.set(null);
            throw e;
        }
    },

    async answer(simId) {
        await apiClient.answerCall(simId);
        const inc = get(incomingCall);
        incomingCall.set(null);
        activeCall.set(inc ? { ...inc, event_type: 'call_answered' } : null);
    },

    async hangup(simId) {
        await apiClient.hangupCall(simId);
        incomingCall.set(null);
        activeCall.set(null);
        stopCallStatusPolling();
        await this.refreshLog();
    },

    async refreshLog(simId = null) {
        try {
            const res = await apiClient.getCallLog(simId, 50);
            callLog.set(res.data?.data ?? []);
        } catch (e) {
            console.error('Failed to load call log:', e);
        }
    },
};

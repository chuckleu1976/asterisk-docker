// api.js
import FetchApi from './request';

/**
 * Encapsulates all API calls, automatically handles authentication and global errors
 */
class ApiClient {
    /**
     * Check authentication validity
     */
    async checkAuth() {
        try {
            const response = await FetchApi.get('/api/check');
            return response.status === 204;
        } catch (error) {
            if (error.status === 401) {
                localStorage.removeItem('auth');
                sessionStorage.removeItem('auth');
                window.location.reload();
            }
            return false;
        }
    }    /**
     * Get paginated SMS list
     * @param {number} [page=1] - Page number
     * @param {number} [perPage=10] - Number of items per page
     * @param {number|null} [contactId=null] - Optional contact ID (for filtering specific contacts)
     * @param {AbortSignal} [signal=null] - Optional AbortSignal to cancel the request
     */
    async getSmsPaginated(page = 1, perPage = 10, contactId = null, signal = null) {
        const params = {
            page: page,
            per_page: perPage,
            contact_id: contactId
        };

        return FetchApi.get('/api/sms', params, undefined, { signal });
    }

    /**
     * Get inbox (received) or sent messages as a flat list with contact_name resolved.
     * @param {'inbox'|'sent'} direction
     * @param {number} page
     * @param {number} perPage
     */
    async getSmsByDirection(direction, page = 1, perPage = 100) {
        return FetchApi.get('/api/sms', { direction, page, per_page: perPage });
    }    /**
     * Send an SMS
     * @param {string} simId - Modem ID
     * @param {object} contact - Target phone number
     * @param {string} message - SMS content
     * @param {boolean} new_message - Whether to send a new message
     */
    async sendSms(simId, contact, message, new_message) {
        const payload = { sim_id: simId, contact, message, new: new_message };
        return FetchApi.post('/api/sms', payload)
    }

    /**
     * @param {any} simId
     */
    async getSimInfo(simId) {
        return await FetchApi.get(`/api/sims/${simId}/info`);
    }

    /**
     * Get all SIM dynamic information
     */
    async getAllSimsInfo() {
        return FetchApi.get('/api/sims/info');
    }

    /**
     * @param {any} simId
     */
    async refreshSms(simId) {
        return FetchApi.get(`/api/sims/${simId}/refresh`)
    }

    async getConversation() {
        return FetchApi.get('/api/conversation')
    }

    async markConversationAsReadAndGetLatest(contactId) {
        return FetchApi.post(`/api/conversations/${contactId}/unread`);
    }

    /**
     * Get all SIM cards information
     */
    async getAllSimCards() {
        return FetchApi.get('/api/sim-cards', {}, 'application/json', {});
    }

    /**
     * Get SMS recv/sent counts grouped by SIM
     */
    async getSimStats() {
        return FetchApi.get('/api/sims/stats');
    }

    /**
     * Update SIM card alias
     * @param {number} simId - SIM card ID
     * @param {string} alias - New alias
     */
    async updateSimCardAlias(simId, alias) {
        const payload = { alias };
        return FetchApi.put(`/api/sim-cards/${simId}/alias`, payload, {}, 'application/json');
    }

    /**
     * Update SIM card phone number
     * @param {number} simId - SIM card ID
     * @param {string} phoneNumber - New phone number
     */
    async updateSimCardPhoneNumber(simId, phoneNumber) {
        const payload = { phone_number: phoneNumber };
        return FetchApi.put(`/api/sim-cards/${simId}/phone`, payload, {}, 'application/json');
    }

    // ── Voice call methods ──────────────────────────────────────────────────

    async makeCall(simId, phone) {
        return FetchApi.post('/api/calls/make', { sim_id: simId, phone });
    }

    async answerCall(simId) {
        return FetchApi.post('/api/calls/answer', { sim_id: simId });
    }

    async hangupCall(simId) {
        return FetchApi.post('/api/calls/hangup', { sim_id: simId });
    }

    async getCallLog(simId = null, limit = 50, offset = 0) {
        const params = { limit, offset };
        if (simId) params.sim_id = simId;
        return FetchApi.get('/api/calls', params);
    }
}

// Export as a singleton
export const apiClient = new ApiClient();

// api.js
import FetchApi from './request';

const DEFAULT_AUTH_TOKEN = 'YWRtaW46MTIzNDU2'; // admin:123456

function buildBasicAuthHeader() {
    const auth = sessionStorage.getItem('auth') || localStorage.getItem('auth');
    if (!auth) {
        return { Authorization: `Basic ${DEFAULT_AUTH_TOKEN}` };
    }

    try {
        const { token } = JSON.parse(auth);
        return token
            ? { Authorization: `Basic ${token}` }
            : { Authorization: `Basic ${DEFAULT_AUTH_TOKEN}` };
    } catch (_) {
        return { Authorization: `Basic ${DEFAULT_AUTH_TOKEN}` };
    }
}

/**
 * Encapsulates all API calls, automatically handles authentication and global errors
 */
class ApiClient {
    /**
     * Check authentication validity
     */
    async checkAuth() {
        const controller = new AbortController();
        const timeoutId = setTimeout(() => controller.abort(), 5000);
        try {
            const response = await FetchApi.get('/api/check', {}, undefined, { signal: controller.signal });
            return response.status === 204;
        } catch (error) {
            if (error.status === 401) {
                localStorage.removeItem('auth');
                sessionStorage.removeItem('auth');
                window.location.reload();
            }
            if (error?.name === 'AbortError') {
                console.warn('Auth check timed out after 5s, falling back to unauthenticated state.');
            }
            return false;
        } finally {
            clearTimeout(timeoutId);
        }
    }

    /**
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
    }

    /** Delete one SMS; optional scope prevents deleting a row from another SIM/phone. */
    async deleteSms(id, simId = null, phoneNumber = null) {
        const query = {};
        if (simId) query.sim_id = simId;
        if (phoneNumber) query.phone_number = phoneNumber;
        return FetchApi.delete(`/api/sms/${id}`, query);
    }

    /** Delete all SMS for exactly one SIM or phone number. */
    async deleteAllSms({ simId = null, phoneNumber = null } = {}) {
        const query = {};
        if (simId) query.sim_id = simId;
        if (phoneNumber) query.phone_number = phoneNumber;
        return FetchApi.delete('/api/sms', query);
    }

    /**
     * Send an SMS
     * @param {string} simId - Modem ID
     * @param {object} contact - Target phone number
     * @param {string} message - SMS content
     * @param {boolean} new_message - Whether to send a new message
     * @param {'pdu'|'text'} [smsFormat='pdu'] - SMS modem send format
     */
    async sendSms(
        simId,
        contact,
        message,
        new_message,
        smsFormat = 'pdu',
        statusReportEnabled = false
    ) {
        const payload = {
            sim_id: simId,
            contact,
            message,
            new: new_message,
            sms_format: smsFormat,
            status_report_enabled: statusReportEnabled,
        };
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

    /**
     * Write the phone number into the SIM card via AT commands and persist it in the DB.
     * @param {string} simId - SIM card ID (ICCID)
     * @param {string} phoneNumber - Phone number to write
     */
    async setSimPhoneNumber(simId, phoneNumber) {
        const payload = { phone_number: phoneNumber };
        return FetchApi.post(`/api/sims/${simId}/phone`, payload, {}, 'application/json');
    }

    /**
     * Force network registration (Quectel scancontrol/cops sequence) on selected SIMs.
     * @param {string[]} simIds - SIM card IDs (ICCID)
     */
    async forceRegister(simIds) {
        return FetchApi.post('/api/sims/force-register', { sim_ids: simIds }, {}, 'application/json');
    }

    /**
     * Reboot selected SIM modems, wait 10 seconds, then retry registration to 46001.
     * @param {string[]} simIds - SIM card IDs (ICCID)
     */
    async reRegister(simIds) {
        return FetchApi.post('/api/sims/re-register', { sim_ids: simIds }, {}, 'application/json');
    }

    /**
     * Release and fully reinitialize selected physical COM ports.
     * @param {string[]} comPorts
     */
    async restartPorts(comPorts) {
        return FetchApi.post('/api/sims/restart-ports', { com_ports: comPorts }, {}, 'application/json');
    }

    // ── 火狐狸 platform integration ─────────────────────────────────────────

    /**
     * Get the stored 火狐狸 platform API key.
     */
    async getFirefoxApiKey() {
        return FetchApi.get('/api/settings/firefox-api-key');
    }

    /**
     * Save the 火狐狸 platform API key.
     * @param {string} apiKey
     */
    async setFirefoxApiKey(apiKey) {
        return FetchApi.put('/api/settings/firefox-api-key', { api_key: apiKey }, {}, 'application/json');
    }

    /**
     * Get the list of supported country codes for the 火狐狸 platform.
     */
    async getFirefoxCountries() {
        return FetchApi.get('/api/firefox/countries');
    }

    /**
     * Upload selected SIM phone numbers to the 火狐狸 platform.
     * @param {string[]} simIds
     * @param {string} countryId
     */
    async uploadToFirefox(simIds, countryId) {
        return FetchApi.post('/api/firefox/upload', { sim_ids: simIds, country_id: countryId }, {}, 'application/json');
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

    // ── Phone number management ─────────────────────────────────────────────

    async importPhoneNumbers(entries) {
        return FetchApi.post('/api/phone-numbers/import', { entries });
    }

    async barcodeScan(iccid, msisdn) {
        return FetchApi.post('/api/phone-numbers/barcode-scan', { iccid, msisdn });
    }

    async getBarcodeScans() {
        return FetchApi.get('/api/phone-numbers/barcode-scans');
    }

    async clearBarcodeScans() {
        return FetchApi.delete('/api/phone-numbers/barcode-scans');
    }

    async importBarcodeScans() {
        return FetchApi.post('/api/phone-numbers/barcode-scans/import');
    }

    async startCallExchange() {
        return FetchApi.post('/api/phone-numbers/call-exchange');
    }

    async startSmsExchange() {
        return FetchApi.post('/api/phone-numbers/sms-exchange');
    }

    async sendUssdBatch(code) {
        return FetchApi.post('/api/phone-numbers/ussd', { code });
    }

    async getPhoneNumberStatus() {
        return FetchApi.get('/api/phone-numbers/status');
    }

    // ── Firefox platform delete ──────────────────────────────────────────

    /**
     * Delete all phone numbers from the 火狐狸 platform.
     */
    async deleteAllFromPlatform() {
        return FetchApi.post('/api/firefox/delete-all', {}, {}, 'application/json');
    }

    /**
     * Get all platform items tracked from the wait list.
     */
    async getFirefoxPlatformItems() {
        return FetchApi.get('/api/firefox/platform-items');
    }

    /**
     * Get the live wait-list directly from the platform.
     */
    async getFirefoxWaitList() {
        return FetchApi.get('/api/firefox/wait-list');
    }

    /**
     * Get detail of a platform item, including its SMS list.
     * @param {string} itemId
     */
    async getFirefoxPlatformItemDetail(itemId, simId = null) {
        const query = simId ? { sim_id: simId } : {};
        return FetchApi.get(`/api/firefox/platform-items/${itemId}`, query);
    }

    /**
     * Get aggregated platform statistics (SMS count per item/phone).
     */
    async getFirefoxPlatformStatistics() {
        return FetchApi.get('/api/firefox/platform-statistics');
    }

    /**
     * Get money-page statistics grouped by COM port / SIM.
     */
    async getFirefoxMoneyStats() {
        return FetchApi.get('/api/firefox/money-stats');
    }

    /**
     * Get item options for money page selector (supports keyword search by item id/name).
     * @param {string} [keyword='']
     * @param {number} [limit=200]
     */
    async getFirefoxMoneyItems(keyword = '', limit = 200) {
        return FetchApi.get('/api/firefox/money-items', { keyword, limit });
    }

    /**
     * Get successful uploaded SMS count for a selected item.
     * @param {string} itemId
     */
    async getFirefoxMoneyItemEarning(itemId) {
        return FetchApi.get('/api/firefox/money-item-earning', { item_id: itemId });
    }

    /**
     * Get the platform (大厅) reference price breakdown by country for an item.
     * @param {string} itemId
     */
    async getFirefoxMoneyItemPlatformPrices(itemId) {
        return FetchApi.get('/api/firefox/money-item-platform-prices', { item_id: itemId });
    }

    /**
     * Persist the unit price for a money-page item and recount its earning.
     * @param {string} itemId
     * @param {number} itemUprice
     */
    async updateFirefoxMoneyItemPrice(itemId, itemUprice) {
        return FetchApi.post(
            '/api/firefox/money-item-price',
            { item_id: itemId, item_uprice: itemUprice },
            {},
            'application/json'
        );
    }

    /**
     * Get recent SMS detail (time/status/content/platform response/money) for a platform item.
     * @param {string} itemId
     */
    async getFirefoxMoneySmsDetailByItem(itemId) {
        return FetchApi.get('/api/firefox/money-sms-detail', { item_id: itemId });
    }

    /**
     * Get recent SMS detail (time/status/content/platform response/money) for a single SIM/phone number.
     * @param {string} simId
     */
    async getFirefoxMoneySmsDetailBySim(simId) {
        return FetchApi.get('/api/firefox/money-sms-detail', { sim_id: simId });
    }

    /**
     * Get grouped platform rejection reasons from failed SMS attempts.
     */
    async getFirefoxPlatformRejectionReasons() {
        return FetchApi.get('/api/firefox/platform-rejection-reasons');
    }

    // ── MMS (EC20/EC25 via AT+QMMSEDIT/AT+QMMSEND) ──────────────────────────

    /**
     * Enqueue a new MMS send job. Returns { id, status: "queued" } immediately;
     * the actual send happens asynchronously in the background worker.
     * @param {{sim_id: string, to: string, subject?: string, attachments?: {filename: string, content_type?: string, base64: string}[]}} payload
     */
    async sendMms(payload) {
        return FetchApi.post('/api/mms', payload);
    }

    /**
     * Get paginated MMS job history.
     * @param {number} [page=1]
     * @param {number} [perPage=20]
     */
    async getMmsPaginated(page = 1, perPage = 20) {
        return FetchApi.get('/api/mms', { page, per_page: perPage });
    }

    /**
     * Get a single MMS job with its attachment metadata.
     * @param {string} id
     */
    async getMmsDetail(id) {
        return FetchApi.get(`/api/mms/${id}`);
    }

    /**
     * Get the stored MMS network profile (APN/MMSC/proxy) for a SIM card.
     * @param {string} simId
     */
    async getMmsProfile(simId) {
        return FetchApi.get(`/api/sim-cards/${simId}/mms-profile`);
    }

    /**
     * Update the MMS network profile for a SIM card.
     * @param {string} simId
     * @param {{apn?: string, mmsc?: string, proxy_host?: string, proxy_port?: number}} payload
     */
    async setMmsProfile(simId, payload) {
        return FetchApi.put(`/api/sim-cards/${simId}/mms-profile`, payload, {}, 'application/json');
    }

    /**
     * Get paginated MMS inbox notifications (phase 1: detected WAP-push
     * notifications; content fetch/decode is a later phase, so status will
     * read "notified" for now).
     * @param {number} [page=1]
     * @param {number} [perPage=20]
     */
    async getMmsInboxPaginated(page = 1, perPage = 20) {
        return FetchApi.get('/api/mms/inbox', { page, per_page: perPage });
    }

    /**
     * Get a single MMS inbox notification.
     * @param {string} id
     */
    async getMmsInboxDetail(id) {
        return FetchApi.get(`/api/mms/inbox/${id}`);
    }

    /**
     * Fetch a decoded MMS inbox part's raw bytes as a Blob (image, SMIL, text, ...).
     * Uses a plain authenticated fetch rather than FetchApi since the response
     * body is binary, not JSON.
     * @param {string} inboxId
     * @param {string} partId
     * @returns {Promise<Blob>}
     */
    async getMmsInboxPartBlob(inboxId, partId) {
        const headers = buildBasicAuthHeader();
        const response = await fetch(`/api/mms/inbox/${inboxId}/parts/${partId}`, { headers });
        if (!response.ok) {
            throw new Error(`Failed to fetch MMS part (${response.status})`);
        }
        return response.blob();
    }

    // ── eSIM management ──────────────────────────────────────────────────────

    /** List eSIM-capable ports and their current mode/session status. */
    async esimListPorts() {
        return FetchApi.get('/api/esim/ports');
    }

    /** Enter eSIM-management mode on a port (acquires the machine-wide lock). */
    async esimEnter(com) {
        return FetchApi.post(`/api/esim/${com}/session/enter`);
    }

    /** Exit eSIM-management mode and release the lock. */
    async esimExit(com) {
        return FetchApi.post(`/api/esim/${com}/session/exit`);
    }

    /** Abnormal-state recovery (ath0). */
    async esimReset(com) {
        return FetchApi.post(`/api/esim/${com}/reset`);
    }

    /** Read eUICC chip info (EID, default SM-DP+). */
    async esimChip(com) {
        return FetchApi.get(`/api/esim/${com}/chip`);
    }

    /** List installed profiles. */
    async esimProfiles(com) {
        return FetchApi.get(`/api/esim/${com}/profiles`);
    }

    /**
     * Download a profile.
     * @param {string} com
     * @param {{smdp?:string, matching_id?:string, confirmation_code?:string, imei?:string, activation_code?:string}} body
     */
    async esimDownload(com, body) {
        return FetchApi.post(`/api/esim/${com}/profiles/download`, body);
    }

    async esimEnable(com, iccid, refreshFlag = true) {
        return FetchApi.post(`/api/esim/${com}/profiles/enable`, { iccid, refresh_flag: refreshFlag });
    }

    async esimDisable(com, iccid, refreshFlag = true) {
        return FetchApi.post(`/api/esim/${com}/profiles/disable`, { iccid, refresh_flag: refreshFlag });
    }

    async esimDelete(com, iccid) {
        return FetchApi.post(`/api/esim/${com}/profiles/delete`, { iccid });
    }

    async esimNickname(com, iccid, nickname) {
        return FetchApi.post(`/api/esim/${com}/profiles/nickname`, { iccid, nickname });
    }

    async esimNotifications(com) {
        return FetchApi.get(`/api/esim/${com}/notifications`);
    }

    async esimNotificationProcess(com, seq, remove = false) {
        return FetchApi.post(`/api/esim/${com}/notifications/process`, { seq, remove });
    }

    /** One-shot enter → download → enable → notify → exit flow. */
    async esimProvision(com, body) {
        return FetchApi.post(`/api/esim/${com}/provision`, body);
    }

    /** Scan the server's eSIM source folder for QR/text activation codes. */
    async esimScanSources() {
        return FetchApi.get('/api/esim/sources');
    }

    /** Upload QR images / text files and get back parsed activation codes. */
    async esimUploadSources(files) {
        const form = new FormData();
        for (const f of files) form.append('files', f, f.name);
        const res = await fetch('/api/esim/sources/upload', {
            method: 'POST',
            headers: buildBasicAuthHeader(),
            body: form,
        });
        const data = await res.json().catch(() => null);
        if (!res.ok) throw { status: res.status, data };
        return { data };
    }

    /** Start a sequential batch job (op = download|delete|activate|deactivate). */
    async esimStartBatch(payload) {
        return FetchApi.post('/api/esim/batch', payload);
    }

    /** Current/last batch job snapshot. */
    async esimGetBatch() {
        return FetchApi.get('/api/esim/batch');
    }

    /** Request cancellation of the running batch job. */
    async esimCancelBatch() {
        return FetchApi.post('/api/esim/batch/cancel');
    }
}

// Export as a singleton
export const apiClient = new ApiClient();

import { writable } from 'svelte/store';
import { apiClient } from '../js/api.js';

function comOrder(port) {
    const match = String(port ?? '').match(/(\d+)\s*$/);
    return match ? Number(match[1]) : 9999;
}

// SIM cards store
export const simCards = writable([]);

// Loading state
export const simCardsLoading = writable(false);

// Functions to manage SIM cards
export const simCardActions = {
    // Load all SIM cards
    async loadAll() {
        simCardsLoading.set(true);
        try {
            const [cardsRes, infoRes] = await Promise.all([
                apiClient.getAllSimCards().catch(() => ({ data: [] })),
                apiClient.getAllSimsInfo().catch(() => ({ data: [] })),
            ]);
            const cards = Array.isArray(cardsRes?.data) ? cardsRes.data : [];
            const infos = (Array.isArray(infoRes?.data) ? infoRes.data : [])
                .filter((info) => info?.sim_id && info.has_sim !== false);
            const cardById = new Map(cards.map((card) => [card.id, card]));
            const merged = infos.map((info) => {
                const card = cardById.get(info.sim_id) ?? {};
                cardById.delete(info.sim_id);
                return {
                    ...card,
                    id: info.sim_id,
                    imsi: card.imsi || info.imsi || null,
                    phone_number: card.phone_number || info.phone_number || null,
                    alias: card.alias || null,
                    com_port: info.com_port || card.com_port || null,
                };
            });
            merged.push(...cardById.values());
            merged.sort((a, b) => comOrder(a.com_port) - comOrder(b.com_port));
            simCards.set(merged);
        } catch (error) {
            console.error('Failed to load SIM cards:', error);
            simCards.set([]);
        } finally {
            simCardsLoading.set(false);
        }
    },

    // Update SIM card alias
    async updateAlias(simId, alias) {
        try {
            await apiClient.updateSimCardAlias(simId, alias);
            // Refresh the list after update
            await this.loadAll();
            return true;
        } catch (error) {
            console.error('Failed to update SIM card alias:', error);
            return false;
        }
    },

    // Update SIM card phone number
    async updatePhoneNumber(simId, phoneNumber) {
        try {
            await apiClient.updateSimCardPhoneNumber(simId, phoneNumber);
            // Refresh the list after update
            await this.loadAll();
            return true;
        } catch (error) {
            console.error('Failed to update SIM card phone number:', error);
            return false;
        }
    }
};
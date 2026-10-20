import { writable } from 'svelte/store';
export const user = writable(null);
export const isAuthenticated = writable(false);
export const isAuthLoading = writable(true);

function signedOut() {
    user.set(null);
    isAuthenticated.set(false);
    sessionStorage.removeItem('remote_esim_request_id');
    sessionStorage.removeItem('auth');
    localStorage.removeItem('auth');
}

window.addEventListener('gateway-auth-expired', signedOut);

export async function initAuth() {
    isAuthLoading.set(true);
    try {
        const response = await fetch('/api/auth/me', { credentials: 'same-origin', cache: 'no-store' });
        if (!response.ok) throw new Error('Session unavailable');
        user.set(await response.json());
        isAuthenticated.set(true);
    } catch {
        signedOut();
    } finally {
        isAuthLoading.set(false);
    }
}

export async function login(username, password) {
    const response = await fetch('/api/auth/login', {
        method: 'POST', credentials: 'same-origin', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ username, password }),
    });
    const result = await response.json().catch(() => ({}));
    if (!response.ok) return { success: false, error: result.error || 'Login failed' };
    signedOut();
    user.set(result);
    isAuthenticated.set(true);
    return { success: true };
}

export async function logout() {
    const response = await fetch('/api/auth/logout', { method: 'POST', credentials: 'same-origin' });
    if (!response.ok && response.status !== 401) throw new Error('Logout unavailable; try again');
    signedOut();
}

initAuth();
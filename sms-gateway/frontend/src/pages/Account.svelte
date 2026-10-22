<script>
  import { onMount } from 'svelte';
  import Icon from '@iconify/svelte';
  import { user, logout, initAuth } from '../stores/auth.js';
  import { t } from '../js/i18n.js';
  import { apiClient } from '../js/api.js';
  import { displayPort, normalizePort } from '../js/modem.js';

  let { onManage = () => {}, onDashboard = () => {} } = $props();
  let tokens = $state([]);
  let error = $state('');
  let operatorPhones = $state({});
  let busy = $state(false);
  let tokenName = $state('');
  let expiry = $state('');
  let createdToken = $state('');
  const tokenPermissions = $derived([...new Set(($user?.rentals ?? []).flatMap(rental => rental.permissions))].sort());

  async function api(path = '', method = 'GET', body = undefined) {
    const response = await fetch('/api/auth/tokens' + path, {
      method, credentials: 'same-origin', cache: 'no-store', headers: { 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const data = await response.json().catch(() => null);
    if (!response.ok) {
      if (response.status === 401) window.dispatchEvent(new Event('gateway-auth-expired'));
      throw new Error(data?.error || 'Request failed');
    }
    return data;
  }

  async function load() {
    try { tokens = await api(); } catch (problem) { error = problem.message; }
  }

  async function loadOperatorPhones() {
    try {
      const [infoRes, cardsRes] = await Promise.all([
        apiClient.getAllSimsInfo(),
        apiClient.getAllSimCards(),
      ]);
      const infos = Array.isArray(infoRes) ? infoRes : (infoRes?.data ?? []);
      const cards = Array.isArray(cardsRes) ? cardsRes : (cardsRes?.data ?? []);
      const cardMap = Object.fromEntries(cards.map((card) => [card.id, card]));
      const next = {};
      for (const info of infos) {
        const card = cardMap[info.sim_id] ?? {};
        const phone = String(card.phone_number ?? info.phone_number ?? info.msisdn ?? '').trim();
        if (info.com_port && phone) next[normalizePort(info.com_port)] = phone;
      }
      operatorPhones = next;
    } catch (problem) {
      if (!error) error = problem.message;
    }
  }

  function operatorMsisdn(rental) {
    const fromCard = String(rental?.sim_msisdn ?? rental?.msisdn ?? '').trim();
    const fromLive = operatorPhones[normalizePort(rental?.com_port)] ?? '';
    return fromCard || fromLive || '—';
  }

  async function createToken(event) {
    event.preventDefault(); busy = true; error = ''; createdToken = '';
    try {
      const result = await api('', 'POST', { name: tokenName, expires_at: new Date(expiry).toISOString(), permissions: tokenPermissions });
      createdToken = result.token; tokenName = ''; await load();
    } catch (problem) { error = problem.message; } finally { busy = false; }
  }

  async function revoke(id) {
    busy = true;
    try { await api('/' + encodeURIComponent(id), 'DELETE'); await load(); }
    catch (problem) { error = problem.message; } finally { busy = false; }
  }

  async function signOut() {
    try { await logout(); } catch (problem) { error = problem.message; }
  }

  onMount(() => {
    load();
    loadOperatorPhones();
  });
</script>

<div class="flex flex-col h-dvh w-screen bg-gray-50 dark:bg-zinc-950 text-sm font-sans">
  <header class="flex items-center justify-between gap-3 px-4 py-3 bg-white dark:bg-zinc-900 border-b border-gray-200 dark:border-zinc-800 shadow-sm shrink-0">
    <div class="flex min-w-0 items-center gap-3">
      <button
        onclick={onDashboard}
        class="inline-flex items-center gap-1.5 h-9 px-2.5 rounded-full bg-blue-600 text-white
               shadow-sm shadow-blue-600/30 hover:bg-blue-700 transition"
        aria-label={$t('btn_back')}
      >
        <Icon icon="carbon:arrow-left" class="w-4 h-4" />
        <span class="text-xs font-semibold">{$t('btn_back')}</span>
      </button>
      <Icon icon="carbon:catalog" class="w-5 h-5 text-gray-500 dark:text-gray-400" />
      <h1 class="text-base font-semibold text-gray-800 dark:text-gray-100">{$t('rentals_title')}</h1>
    </div>
    <div class="flex shrink-0 items-center gap-2 text-sm">
      <span class="hidden sm:inline max-w-48 truncate text-gray-600 dark:text-gray-300">{$user?.username}</span>
      <button
        onclick={initAuth}
        title="Refresh rentals"
        aria-label="Refresh rentals"
        class="p-2 rounded hover:bg-gray-100 dark:hover:bg-zinc-800"
      >
        <Icon icon="carbon:renew" class="w-5 h-5" />
      </button>
      <button
        onclick={signOut}
        title={$t('btn_logout')}
        aria-label={$t('btn_logout')}
        class="p-2 rounded hover:bg-gray-100 dark:hover:bg-zinc-800"
      >
        <Icon icon="carbon:logout" class="w-5 h-5" />
      </button>
    </div>
  </header>
  <main class="flex-1 overflow-auto p-5 space-y-8">
    {#if error}<p role="alert" class="text-sm text-red-600 break-words">{error}</p>{/if}
    <section>
      <div class="overflow-x-auto"><table class="w-full text-left text-sm border-collapse">
        <thead class="border-b dark:border-zinc-700 text-gray-500"><tr><th class="p-3">Client</th><th class="p-3">Physical Mapping</th><th class="p-3">Expires</th><th class="p-3">MSISDN</th></tr></thead><tbody>
          {#each $user?.rentals ?? [] as rental (rental.id)}<tr class="border-b dark:border-zinc-700"><td class="p-3 whitespace-nowrap">{displayPort(rental.com_port)}</td><td class="p-3 whitespace-nowrap">B{rental.bank_id}:{rental.bank_slot} &#8594; C{rental.client_id}:{rental.client_slot}</td><td class="p-3 whitespace-nowrap">{new Date(rental.expires_at).toLocaleString()}</td><td class="p-3 font-mono whitespace-nowrap">{operatorMsisdn(rental)}</td></tr>
          {:else}<tr><td colspan="4" class="p-5 text-gray-500">No active rentals.</td></tr>{/each}
        </tbody></table></div>
      <button onclick={onManage} disabled={!$user?.rentals?.length} class="mt-4 inline-flex items-center gap-2 rounded bg-emerald-700 text-white px-4 py-2 disabled:opacity-40"><Icon icon="carbon:chip" width="18" />Manage eSIMs</button>
    </section>
    <section class="border-t dark:border-zinc-700 pt-6">
      <h2 class="text-xl font-semibold mb-4">API Tokens</h2>
      <form onsubmit={createToken} class="flex flex-wrap items-end gap-3">
        <label class="flex flex-col gap-1 text-sm">Name<input bind:value={tokenName} maxlength="128" required class="rounded border bg-white dark:bg-zinc-800 px-3 py-2" /></label>
        <label class="flex flex-col gap-1 text-sm">Expires<input type="datetime-local" bind:value={expiry} required class="rounded border bg-white dark:bg-zinc-800 px-3 py-2" /></label>
        <button disabled={busy || !tokenPermissions.length} class="inline-flex items-center gap-2 rounded bg-gray-900 dark:bg-zinc-700 text-white px-3 py-2 disabled:opacity-40"><Icon icon="carbon:add" width="18" />Create Token</button>
      </form>
      {#if createdToken}<label class="block mt-3 text-sm">New Token<input readonly type="text" value={createdToken} autocomplete="off" class="block w-full mt-1 rounded border px-3 py-2 bg-white dark:bg-zinc-800 font-mono" /></label><button onclick={() => createdToken = ''} class="mt-2 text-sm underline">Dismiss</button>{/if}
      <div class="overflow-x-auto mt-4"><table class="w-full text-left text-sm"><thead class="border-b dark:border-zinc-700 text-gray-500"><tr><th class="p-3">Name</th><th class="p-3">Expires</th><th class="p-3">Status</th><th class="p-3">Actions</th></tr></thead><tbody>
        {#each tokens as token (token.id)}<tr class="border-b dark:border-zinc-700"><td class="p-3 break-all">{token.name}</td><td class="p-3 whitespace-nowrap">{new Date(token.expires_at).toLocaleString()}</td><td class="p-3">{!token.active ? 'Revoked' : new Date(token.expires_at) <= new Date() ? 'Expired' : 'Active'}</td><td class="p-3"><button onclick={() => revoke(token.id)} disabled={busy || !token.active} title="Revoke token" aria-label="Revoke token" class="p-2 text-red-600 disabled:opacity-30"><Icon icon="carbon:trash-can" width="18" /></button></td></tr>{/each}
      </tbody></table></div>
    </section>
  </main>
</div>
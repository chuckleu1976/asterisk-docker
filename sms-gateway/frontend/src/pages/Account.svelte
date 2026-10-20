<script>
  import { onMount } from 'svelte';
  import Icon from '@iconify/svelte';
  import { user, logout, initAuth } from '../stores/auth.js';
  import appLogo from '../../../icon/cow.png';
  import { t } from '../js/i18n.js';
  import { displayPort } from '../js/modem.js';
  import MsisdnEditor from '../components/MsisdnEditor.svelte';

  let { onManage = () => {}, onDashboard = () => {} } = $props();
  let tokens = $state([]);
  let error = $state('');
  let busy = $state(false);
  let tokenName = $state('');
  let expiry = $state('');
  let permissions = $state(['esim:read']);
  let createdToken = $state('');
  const available = $derived([...new Set(($user?.rentals ?? []).flatMap(rental => rental.permissions))].sort());

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

  async function createToken(event) {
    event.preventDefault(); busy = true; error = ''; createdToken = '';
    try {
      const result = await api('', 'POST', { name: tokenName, expires_at: new Date(expiry).toISOString(), permissions });
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

  onMount(load);
</script>

<div class="h-dvh overflow-auto bg-gray-50 dark:bg-zinc-900 text-gray-900 dark:text-gray-100">
  <header class="flex flex-wrap items-center justify-between gap-4 border-b border-gray-200 dark:border-zinc-700 px-5 py-4">
    <div class="flex min-w-0 items-center gap-3"><img src={appLogo} alt="" class="w-9 h-9 shrink-0 object-contain" /><h1 class="text-lg font-semibold">{$t('sim_dashboard_title')}</h1></div>
    <div class="flex flex-wrap items-center gap-3 text-sm"><span class="break-all">{$user?.username}</span>
      {#if $user?.is_admin}<button onclick={onDashboard} class="rounded border px-3 py-2">Dashboard</button>{/if}
      <button onclick={signOut} title="Sign out" aria-label="Sign out" class="p-2 rounded hover:bg-gray-200 dark:hover:bg-zinc-800"><Icon icon="carbon:logout" width="20" /></button>
    </div>
  </header>
  <main class="max-w-6xl mx-auto p-5 space-y-8">
    {#if error}<p role="alert" class="text-sm text-red-600 break-words">{error}</p>{/if}
    <section>
      <div class="flex items-center justify-between gap-3 mb-4"><h2 class="text-xl font-semibold">My Rentals</h2><button onclick={initAuth} title="Refresh rentals" aria-label="Refresh rentals" class="p-2"><Icon icon="carbon:renew" width="20" /></button></div>
      <div class="overflow-x-auto"><table class="w-full text-left text-sm border-collapse">
        <thead class="border-b dark:border-zinc-700 text-gray-500"><tr><th class="p-3">Client</th><th class="p-3">Physical Mapping</th><th class="p-3">Expires</th><th class="p-3">MSISDN</th><th class="p-3">Permissions</th></tr></thead><tbody>
          {#each $user?.rentals ?? [] as rental (rental.id)}<tr class="border-b dark:border-zinc-700"><td class="p-3 whitespace-nowrap">{displayPort(rental.com_port)}</td><td class="p-3 whitespace-nowrap">B{rental.bank_id}:{rental.bank_slot} &#8594; C{rental.client_id}:{rental.client_slot}</td><td class="p-3 whitespace-nowrap">{new Date(rental.expires_at).toLocaleString()}</td><td class="p-3 min-w-64"><MsisdnEditor {rental} /></td><td class="p-3 min-w-48">{rental.permissions.join(', ')}</td></tr>
          {:else}<tr><td colspan="5" class="p-5 text-gray-500">No active rentals.</td></tr>{/each}
        </tbody></table></div>
      <button onclick={onManage} disabled={!$user?.rentals?.length} class="mt-4 inline-flex items-center gap-2 rounded bg-emerald-700 text-white px-4 py-2 disabled:opacity-40"><Icon icon="carbon:chip" width="18" />Manage eSIMs</button>
    </section>
    <section class="border-t dark:border-zinc-700 pt-6">
      <h2 class="text-xl font-semibold mb-4">API Tokens</h2>
      <form onsubmit={createToken} class="flex flex-wrap items-end gap-3">
        <label class="flex flex-col gap-1 text-sm">Name<input bind:value={tokenName} maxlength="128" required class="rounded border bg-white dark:bg-zinc-800 px-3 py-2" /></label>
        <label class="flex flex-col gap-1 text-sm">Expires<input type="datetime-local" bind:value={expiry} required class="rounded border bg-white dark:bg-zinc-800 px-3 py-2" /></label>
        <button disabled={busy || !permissions.length} class="inline-flex items-center gap-2 rounded bg-gray-900 dark:bg-zinc-700 text-white px-3 py-2 disabled:opacity-40"><Icon icon="carbon:add" width="18" />Create Token</button>
        <fieldset class="flex flex-wrap gap-x-4 gap-y-2 w-full text-sm py-2"><legend class="sr-only">Permissions</legend>{#each available as permission}<label class="flex items-center gap-2"><input type="checkbox" bind:group={permissions} value={permission} />{permission}</label>{/each}</fieldset>
      </form>
      {#if createdToken}<label class="block mt-3 text-sm">New Token<input readonly type="text" value={createdToken} autocomplete="off" class="block w-full mt-1 rounded border px-3 py-2 bg-white dark:bg-zinc-800 font-mono" /></label><button onclick={() => createdToken = ''} class="mt-2 text-sm underline">Dismiss</button>{/if}
      <div class="overflow-x-auto mt-4"><table class="w-full text-left text-sm"><thead class="border-b dark:border-zinc-700 text-gray-500"><tr><th class="p-3">Name</th><th class="p-3">Expires</th><th class="p-3">Status</th><th class="p-3">Actions</th></tr></thead><tbody>
        {#each tokens as token (token.id)}<tr class="border-b dark:border-zinc-700"><td class="p-3 break-all">{token.name}</td><td class="p-3 whitespace-nowrap">{new Date(token.expires_at).toLocaleString()}</td><td class="p-3">{!token.active ? 'Revoked' : new Date(token.expires_at) <= new Date() ? 'Expired' : 'Active'}</td><td class="p-3"><button onclick={() => revoke(token.id)} disabled={busy || !token.active} title="Revoke token" aria-label="Revoke token" class="p-2 text-red-600 disabled:opacity-30"><Icon icon="carbon:trash-can" width="18" /></button></td></tr>{/each}
      </tbody></table></div>
    </section>
  </main>
</div>
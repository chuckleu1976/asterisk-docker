<script>
  import { onMount, onDestroy } from 'svelte';
  import Icon from '@iconify/svelte';
  import { apiClient } from '../js/api.js';
  import { t } from '../js/i18n.js';

  let { onBack = () => {} } = $props();

  let lines = $state([]);
  let drafts = $state({});
  let loading = $state(true);
  let error = $state('');
  let saving = $state('');
  let login = $state(null);

  function sessionLabel(session) {
    if (session === 'logged in') return $t('tg_session_in');
    if (session === 'login needs a code') return $t('tg_session_code');
    if (session === 'login needs a password') return $t('tg_session_2fa');
    return $t('tg_session_out');
  }

  function blankDraft(line) {
    return {
      forward: line.forward ?? '',
      caller: '',
      destination: '',
      routes: (line.routes ?? []).map((route) => ({ ...route })),
    };
  }

  function applyLines(next) {
    const previous = drafts;
    drafts = Object.fromEntries(next.map((line) => {
      const current = previous[line.instance];
      if (current && saving === String(line.instance)) return [line.instance, current];
      return [line.instance, current ?? blankDraft(line)];
    }));
    lines = next;
  }

  async function fetchLines() {
    try {
      const res = await apiClient.getTg2sip();
      const data = res?.data ?? res;
      applyLines(Array.isArray(data?.lines) ? data.lines : []);
      error = '';
    } catch (e) {
      error = e?.data?.error ?? e?.message ?? 'Failed to load Telegram bridge';
    } finally {
      loading = false;
    }
  }

  function messageOf(e) {
    return e?.data?.error ?? e?.message ?? 'Request failed';
  }

  async function saveForward(line) {
    const draft = drafts[line.instance];
    saving = String(line.instance);
    error = '';
    try {
      await apiClient.setTg2sipForward(line.instance, draft.forward.trim());
      drafts[line.instance] = null;
      await fetchLines();
    } catch (e) {
      error = messageOf(e);
    } finally {
      saving = '';
    }
  }

  function addRoute(instance) {
    const draft = drafts[instance];
    const caller = draft.caller.trim();
    const destination = draft.destination.trim();
    if (!caller || !destination) return;
    draft.routes = [...draft.routes, { caller, destination }];
    draft.caller = '';
    draft.destination = '';
  }

  function removeRoute(instance, index) {
    const draft = drafts[instance];
    draft.routes = draft.routes.filter((_, i) => i !== index);
  }

  async function saveRoutes(line) {
    const draft = drafts[line.instance];
    saving = String(line.instance);
    error = '';
    try {
      await apiClient.setTg2sipRoutes(line.instance, draft.routes);
      drafts[line.instance] = null;
      await fetchLines();
    } catch (e) {
      error = messageOf(e);
    } finally {
      saving = '';
    }
  }

  async function setPower(line) {
    saving = String(line.instance);
    error = '';
    try {
      await apiClient.setTg2sipPower(line.instance, line.running ? 'stop' : 'start');
      await fetchLines();
    } catch (e) {
      error = messageOf(e);
    } finally {
      saving = '';
    }
  }

  function openLogin(line) {
    const step = line.session === 'login needs a code'
      ? 'code'
      : line.session === 'login needs a password'
        ? 'password'
        : 'phone';
    login = { instance: line.instance, step, phone: '', code: '', password: '', error: '', busy: false };
  }

  async function submitLogin() {
    if (!login) return;
    login.busy = true;
    login.error = '';
    const body = login.step === 'phone'
      ? { phone: login.phone.trim() }
      : login.step === 'code'
        ? { code: login.code.trim() }
        : { password: login.password };
    try {
      const res = await apiClient.submitTg2sipSession(login.instance, body);
      const data = res?.data ?? res;
      if (data?.error && data.session !== 'login needs a code' && data.session !== 'login needs a password') {
        login.error = data.error;
        login.busy = false;
        return;
      }
      if (data.session === 'login needs a code') {
        login.step = 'code';
        login.error = data.error ?? '';
      } else if (data.session === 'login needs a password') {
        login.step = 'password';
        login.error = data.error ?? '';
      } else if (data.session === 'logged in') {
        login = null;
        await fetchLines();
        return;
      } else {
        login.error = data?.error ?? 'Login did not finish';
      }
    } catch (e) {
      login.error = messageOf(e);
    }
    login.busy = false;
  }

  let timer;
  onMount(() => {
    fetchLines();
    timer = setInterval(fetchLines, 3000);
  });
  onDestroy(() => clearInterval(timer));
</script>

<div class="flex h-dvh w-screen flex-col bg-[#f2f2f2] text-gray-900 dark:bg-zinc-900 dark:text-gray-100">
  <header class="flex items-center justify-between border-b border-gray-200 bg-white px-3 py-2 shadow-sm dark:border-zinc-700 dark:bg-zinc-800">
    <div class="flex items-center gap-2.5">
      <button
        onclick={onBack}
        class="inline-flex items-center gap-1.5 h-8 px-2.5 rounded-full bg-blue-600 text-white shadow-sm shadow-blue-600/30 hover:bg-blue-700 transition"
      >
        <Icon icon="carbon:arrow-left" class="h-3.5 w-3.5" />
        <span class="text-xs font-semibold">{$t('btn_back')}</span>
      </button>
      <h1 class="text-sm font-semibold tracking-wide">{$t('tg_page_title')}</h1>
    </div>
  </header>

  <main class="flex-1 overflow-auto p-4 space-y-4">
    {#if error}
      <p class="rounded-md bg-red-50 px-3 py-2 text-sm text-red-700 dark:bg-red-950 dark:text-red-200">{error}</p>
    {/if}
    {#if loading && lines.length === 0}
      <p class="text-sm text-gray-500">…</p>
    {/if}

    {#each lines as line (line.instance)}
      {@const draft = drafts[line.instance]}
      {@const busy = line.call === 'busy'}
      <section class="rounded-xl border border-gray-200 bg-white p-4 shadow-sm dark:border-zinc-700 dark:bg-zinc-800">
        <div class="flex flex-wrap items-baseline justify-between gap-2">
          <h2 class="text-sm font-semibold">
            {line.hostname}
            {#if line.msisdn}<span class="font-normal text-gray-500"> · {line.msisdn}</span>{/if}
          </h2>
          <p class="text-xs text-gray-500">
            {line.sip === 'registered' ? $t('tg_sip_registered') : $t('tg_sip_down')}
            · {busy ? $t('tg_busy') : $t('tg_idle')}
            · {sessionLabel(line.session)}
          </p>
        </div>
        <p class="mt-1 text-[11px] text-gray-400">{line.container}</p>

        {#if line.session !== 'logged in'}
          <button
            onclick={() => openLogin(line)}
            class="mt-3 inline-flex items-center rounded-md bg-blue-600 px-3 py-1.5 text-xs font-semibold text-white hover:bg-blue-700"
          >
            {$t('tg_login')}
          </button>
        {:else if draft}
          <label class="mt-4 block text-xs font-medium text-gray-600 dark:text-gray-300">
            {$t('tg_inbound')}
            <div class="mt-1 flex gap-2">
              <input
                bind:value={drafts[line.instance].forward}
                disabled={busy}
                placeholder="@alice or +8613800138000"
                class="h-8 flex-1 rounded-md border border-gray-300 bg-white px-2 text-sm dark:border-zinc-600 dark:bg-zinc-900"
              />
              <button
                onclick={() => saveForward(line)}
                disabled={busy || saving === String(line.instance)}
                class="h-8 rounded-md bg-blue-600 px-3 text-xs font-semibold text-white disabled:opacity-50"
              >{$t('tg_save')}</button>
            </div>
          </label>

          <h3 class="mt-4 text-xs font-medium text-gray-600 dark:text-gray-300">{$t('tg_routes')}</h3>
          <div class="mt-2 space-y-1">
            {#each draft.routes as route, index (route.caller + route.destination + index)}
              <div class="flex items-center gap-2 text-sm">
                <span class="w-40 truncate">{route.caller}</span>
                <span class="text-gray-400">→</span>
                <span class="flex-1 truncate">{route.destination}</span>
                <button
                  onclick={() => removeRoute(line.instance, index)}
                  disabled={busy}
                  class="text-xs text-red-600 disabled:opacity-50"
                >{$t('tg_remove')}</button>
              </div>
            {/each}
          </div>
          <div class="mt-2 flex flex-wrap gap-2">
            <input
              bind:value={drafts[line.instance].caller}
              disabled={busy}
              placeholder={$t('tg_add_caller')}
              class="h-8 w-40 rounded-md border border-gray-300 bg-white px-2 text-sm dark:border-zinc-600 dark:bg-zinc-900"
            />
            <input
              bind:value={drafts[line.instance].destination}
              disabled={busy}
              placeholder={$t('tg_add_number')}
              class="h-8 w-44 rounded-md border border-gray-300 bg-white px-2 text-sm dark:border-zinc-600 dark:bg-zinc-900"
            />
            <button
              onclick={() => addRoute(line.instance)}
              disabled={busy}
              class="h-8 rounded-md border border-gray-300 px-3 text-xs dark:border-zinc-600"
            >{$t('tg_add_caller')}</button>
            <button
              onclick={() => saveRoutes(line)}
              disabled={busy || saving === String(line.instance)}
              class="h-8 rounded-md bg-blue-600 px-3 text-xs font-semibold text-white disabled:opacity-50"
            >{$t('tg_save')}</button>
          </div>
          {#if busy}
            <p class="mt-2 text-[11px] text-amber-700 dark:text-amber-300">{$t('tg_busy_locked')}</p>
          {/if}
          <button
            onclick={() => setPower(line)}
            disabled={busy || saving === String(line.instance)}
            class="mt-4 inline-flex items-center rounded-md border border-gray-300 px-3 py-1.5 text-xs font-medium disabled:opacity-50 dark:border-zinc-600"
          >
            {line.running ? $t('tg_stop') : $t('tg_start')}
          </button>
        {/if}
      </section>
    {/each}
  </main>

  {#if login}
    <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 px-4">
      <form
        class="w-full max-w-sm rounded-xl bg-white p-4 shadow-xl dark:bg-zinc-800"
        onsubmit={(e) => { e.preventDefault(); submitLogin(); }}
      >
        <h2 class="text-sm font-semibold">{$t('tg_login')}</h2>
        {#if login.step === 'phone'}
          <label class="mt-3 block text-xs">
            {$t('tg_phone')}
            <input bind:value={login.phone} class="mt-1 h-8 w-full rounded-md border border-gray-300 px-2 text-sm dark:border-zinc-600 dark:bg-zinc-900" />
          </label>
        {:else if login.step === 'code'}
          <label class="mt-3 block text-xs">
            {$t('tg_code')}
            <input bind:value={login.code} class="mt-1 h-8 w-full rounded-md border border-gray-300 px-2 text-sm dark:border-zinc-600 dark:bg-zinc-900" />
          </label>
        {:else}
          <label class="mt-3 block text-xs">
            {$t('tg_2fa')}
            <input type="password" bind:value={login.password} class="mt-1 h-8 w-full rounded-md border border-gray-300 px-2 text-sm dark:border-zinc-600 dark:bg-zinc-900" />
          </label>
        {/if}
        {#if login.error}
          <p class="mt-2 text-xs text-red-600">{login.error}</p>
        {/if}
        <div class="mt-4 flex justify-end gap-2">
          <button type="button" onclick={() => login = null} class="h-8 px-3 text-xs">{$t('tg_cancel')}</button>
          <button type="submit" disabled={login.busy} class="h-8 rounded-md bg-blue-600 px-3 text-xs font-semibold text-white disabled:opacity-50">
            {$t('tg_continue')}
          </button>
        </div>
      </form>
    </div>
  {/if}
</div>

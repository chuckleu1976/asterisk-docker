<script>
  import { onMount } from 'svelte';
  import Icon from '@iconify/svelte';
  import { callLog, callActions, incomingCall, activeCall, callSseConnected } from '../../stores/calls.js';
  import { simCards } from '../../stores/simcards.js';
  import { formatDate } from '../../js/dateFormat.js';
  import { apiClient } from '../../js/api.js';
  import { t } from '../../js/i18n.js';

  let { filterSimId = null } = $props();

  let makeCallPhone = $state('');
  let makeCallSimId = $state('');
  let making = $state(false);
  let makeError = $state('');
  let callMode = $state('');
  let modeError = $state('');

  let sims = $derived($simCards);
  let log = $derived($callLog);
  let filteredLog = $derived(filterSimId ? log.filter(call => call.sim_id === filterSimId) : log);
  let incoming = $derived($incomingCall);
  let active = $derived($activeCall);

  onMount(() => {
    callActions.refreshLog();
    if (filterSimId) {
      makeCallSimId = filterSimId;
      return;
    }
    if (sims.length > 0 && !makeCallSimId) {
      makeCallSimId = sims[0].id;
    }
  });

  $effect(() => {
    if (filterSimId) {
      makeCallSimId = filterSimId;
    }
  });

  $effect(() => {
    if (sims.length > 0 && !makeCallSimId) {
      makeCallSimId = sims[0].id;
    }
  });

  $effect(() => {
    if (!makeCallSimId) return;
    let cancelled = false;
    apiClient.getCallMode(makeCallSimId).then(result => {
      if (!cancelled) { callMode = result.data?.mode ?? 'telegram'; modeError = ''; }
    }).catch(error => {
      if (!cancelled) modeError = error?.message ?? 'Unable to load call routing';
    });
    return () => { cancelled = true; };
  });

  async function changeCallMode(mode) {
    modeError = '';
    try {
      const result = await apiClient.setCallMode(makeCallSimId, mode);
      callMode = result.data?.mode ?? mode;
    } catch (error) {
      modeError = error?.message ?? 'Unable to update call routing';
    }
  }

  async function handleMakeCall() {
    if (!makeCallPhone.trim() || !makeCallSimId) return;
    making = true;
    makeError = '';
    try {
      await callActions.make(makeCallSimId, makeCallPhone.trim());
    } catch (e) {
      makeError = e?.data?.message ?? 'Call failed';
    } finally {
      making = false;
    }
  }

  function statusIcon(status) {
    switch (status) {
      case 'ended': return 'carbon:phone-off';
      case 'missed': return 'carbon:phone-missed';
      case 'ringing': return 'carbon:phone-incoming';
      case 'active': return 'carbon:phone-voice';
      default: return 'carbon:phone';
    }
  }

  function statusColor(status) {
    switch (status) {
      case 'ended': return 'text-zinc-400';
      case 'missed': return 'text-red-400';
      case 'ringing': return 'text-yellow-400';
      case 'active': return 'text-green-400';
      default: return 'text-zinc-400';
    }
  }

  function directionIcon(direction) {
    return direction === 'inbound' ? 'carbon:arrow-down' : 'carbon:arrow-up';
  }

  function durationLabel(started, ended) {
    if (!ended) return '';
    const s = Math.round((new Date(ended) - new Date(started)) / 1000);
    if (s < 60) return `${s}s`;
    return `${Math.floor(s / 60)}m ${s % 60}s`;
  }
</script>

<div class="flex flex-col h-full bg-white dark:bg-zinc-900 overflow-hidden">
  <!-- Header -->
  <div class="px-4 py-3 border-b border-gray-200/70 dark:border-zinc-800 shrink-0">
    <h2 class="text-base font-semibold text-gray-900 dark:text-white">{$t('call_log_title')}</h2>
    {#if filterSimId}
      {@const selected = sims.find((sim) => sim.id === filterSimId)}
      <p class="text-xs text-blue-600 dark:text-blue-400 mt-0.5">
        {selected?.phone_number || filterSimId.slice(-6)}
      </p>
    {/if}
    <p class="text-xs text-gray-500 dark:text-zinc-500 mt-0.5">
      SSE: <span class={$callSseConnected ? 'text-green-500' : 'text-red-400'}>
        {$callSseConnected ? $t('sse_connected') : $t('sse_disconnected')}
      </span>
    </p>
  </div>

  <!-- Make a call -->
  <div class="px-4 py-3 border-b border-gray-200/70 dark:border-zinc-800 shrink-0">
    <p class="text-xs font-medium text-gray-500 dark:text-zinc-400 mb-2 uppercase tracking-wide">{$t('make_a_call')}</p>
    <fieldset class="mb-3">
      <legend class="mb-1 text-xs text-gray-500 dark:text-zinc-400">Call routing</legend>
      <div class="inline-flex max-w-full overflow-hidden rounded border border-gray-200 dark:border-zinc-700">
        <button type="button" aria-pressed={callMode === 'telegram'} onclick={() => changeCallMode('telegram')}
          class="px-3 py-2 text-xs font-medium aria-pressed:bg-gray-900 aria-pressed:text-white dark:aria-pressed:bg-gray-100 dark:aria-pressed:text-gray-900">Telegram</button>
        <button type="button" aria-pressed={callMode === 'plain'} onclick={() => changeCallMode('plain')}
          class="px-3 py-2 text-xs font-medium aria-pressed:bg-gray-900 aria-pressed:text-white dark:aria-pressed:bg-gray-100 dark:aria-pressed:text-gray-900">Plain on line</button>
      </div>
    </fieldset>
    <div class="flex gap-2 items-center flex-wrap">
      <select
        bind:value={makeCallSimId}
        disabled={!!filterSimId}
        class="text-sm rounded-lg border border-gray-200 dark:border-zinc-700 bg-white dark:bg-zinc-800 text-gray-800 dark:text-gray-100 px-2 py-1.5 min-w-0 max-w-[180px] shrink-0"
      >
        {#each sims as sim}
          <option value={sim.id}>{sim.alias ?? sim.phone_number ?? sim.id.slice(0, 8)}</option>
        {/each}
      </select>
      <input
        type="tel"
        placeholder={$t('phone_number_ph')}
        bind:value={makeCallPhone}
        onkeydown={(e) => e.key === 'Enter' && handleMakeCall()}
        class="flex-1 text-sm rounded-lg border border-gray-200 dark:border-zinc-700 bg-white dark:bg-zinc-800 text-gray-800 dark:text-gray-100 px-3 py-1.5 outline-none focus:ring-2 focus:ring-green-500/50"
      />
      <button
        onclick={handleMakeCall}
        disabled={making || !makeCallPhone.trim() || !!active || !!incoming}
        class="shrink-0 w-9 h-9 rounded-full bg-green-500 hover:bg-green-400 active:scale-95 transition flex items-center justify-center disabled:opacity-40"
        aria-label="Call"
      >
        <Icon icon="carbon:phone" class="w-4 h-4 text-white" />
      </button>
    </div>
    {#if makeError || modeError}
      <p role="alert" class="text-xs text-red-500 mt-1">{makeError || modeError}</p>
    {/if}
  </div>

  <!-- Log list -->
  <div class="flex-1 overflow-y-auto divide-y divide-gray-100 dark:divide-zinc-800/70">
    {#if filteredLog.length === 0}
      <div class="flex flex-col items-center justify-center h-full gap-2 text-zinc-400">
        <Icon icon="carbon:phone-off" class="w-10 h-10 opacity-30" />
        <p class="text-sm">{$t('no_calls')}</p>
      </div>
    {:else}
      {#each filteredLog as call (call.id)}
        <div class="flex items-center gap-3 px-4 py-3 hover:bg-gray-50 dark:hover:bg-zinc-800/40 transition-colors">
          <!-- Direction + status icon -->
          <div class="relative shrink-0 w-10 h-10 rounded-full bg-gray-100 dark:bg-zinc-800 flex items-center justify-center">
            <Icon icon={statusIcon(call.status)} class="w-5 h-5 {statusColor(call.status)}" />
            <span class="absolute -bottom-0.5 -right-0.5 w-4 h-4 rounded-full bg-white dark:bg-zinc-900 flex items-center justify-center">
              <Icon icon={directionIcon(call.direction)} class="w-3 h-3 text-zinc-400" />
            </span>
          </div>

          <!-- Info -->
          <div class="flex-1 min-w-0">
            <p class="text-sm font-medium text-gray-900 dark:text-white truncate">
              {call.phone ?? $t('unknown')}
            </p>
            <p class="text-xs text-gray-500 dark:text-zinc-500 truncate">
              {call.sim_id.slice(0, 12)}… · {call.status === 'ringing' ? 'Ringing' : call.status === 'active' ? 'Answered' : call.status === 'ended' ? 'Ended' : call.status === 'missed' ? 'Missed' : call.status}
              {#if call.ended_at && call.status === 'ended'}
                · {durationLabel(call.started_at, call.ended_at)}
              {/if}
            </p>
          </div>

          <!-- Time -->
          <p class="text-xs text-gray-400 dark:text-zinc-500 shrink-0">
            {formatDate(call.started_at)}
          </p>
        </div>
      {/each}
    {/if}
  </div>
</div>

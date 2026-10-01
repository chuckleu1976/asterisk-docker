<script>
  import { onMount, onDestroy } from 'svelte';
  import Icon from '@iconify/svelte';
  import { apiClient } from '../js/api.js';
  import pkg from '../../package.json';
  import { displayPort, getModuleLabel } from '../js/modem.js';
  import { t, lang } from '../js/i18n.js';
  import { getMccCountry } from '../js/country.js';

  const APP_VERSION = pkg?.version ?? '0.0.0';

  // ── State ──────────────────────────────────────────────────────────────────
  let simsInfo   = $state([]);   // from /api/sims/info  (live AT data)
  let simCards   = $state([]);   // from /api/sim-cards  (DB: ICCID/IMSI)
  let simStats   = $state([]);   // from /api/sims/stats (SMS counts)
  let loading    = $state(true);
  let error      = $state('');
  let isFetching = false;
  let selected   = $state(new Set());
  let lastClickedRowIndex = $state(null);
  let hoveredRow = $state(null);
  let showOperatorColumn = $state(true);
  let visibleDataColumnCount = $derived(showOperatorColumn ? 12 : 11);

  // ── Network status map ────────────────────────────────────────────────────
  const netStatusKeys = {
    0: { key: 'net_not_registered', cls: 'text-gray-400' },
    1: { key: 'net_home',           cls: 'text-green-500' },
    2: { key: 'net_searching',      cls: 'text-yellow-400' },
    3: { key: 'net_denied',         cls: 'text-red-500' },
    4: { key: 'net_unknown',        cls: 'text-gray-400' },
    5: { key: 'net_roaming',        cls: 'text-blue-400' },
  };

  function getNetStatus(reg) {
    if (!reg) return { key: null, label: '—', cls: 'text-gray-400' };
    const s = netStatusKeys[reg.status];
    if (s) return s;
    return { key: 'net_code', codeN: reg.status, cls: 'text-gray-400' };
  }

  // RSSI → dBm helper
  function rssiToDbm(rssi) {
    if (rssi == null) return '—';
    if (rssi === 99) return 'N/A';
    return `${-113 + rssi * 2} dBm`;
  }

  function normalizePhoneForDisplay(raw) {
    if (!raw) return '';
    const digits = String(raw).replace(/\D/g, '');
    return digits.replace(/^0+/, '');
  }

  // Signal bar count (0-4)
  function signalBars(rssi) {
    if (rssi == null || rssi === 99) return 0;
    if (rssi >= 20) return 4;
    if (rssi >= 15) return 3;
    if (rssi >= 10) return 2;
    if (rssi >= 5)  return 1;
    return 0;
  }

  function getSimStatusBadge(statusRaw) {
    const status = String(statusRaw ?? '').trim().toUpperCase();
    if (status === 'RECONNECTING') {
      return {
        label: $t('sim_reconnecting'),
        cls: 'bg-amber-100 dark:bg-amber-900/30 text-amber-700 dark:text-amber-300'
      };
    }
    if (status === 'DISCONNECTED') {
      return {
        label: $t('sim_disconnected'),
        cls: 'bg-gray-100 dark:bg-zinc-700 text-gray-500 dark:text-gray-400'
      };
    }
    if (status === 'SIM REMOVED') {
      return {
        label: $t('sim_removed'),
        cls: 'bg-red-100 dark:bg-red-900/30 text-red-700 dark:text-red-400'
      };
    }
    if (status === 'NO SIM') {
      return {
        label: $t('sim_removed'),
        cls: 'bg-red-100 dark:bg-red-900/30 text-red-700 dark:text-red-400'
      };
    }
    return {
      label: statusRaw,
      cls: 'bg-red-100 dark:bg-red-900/30 text-red-700 dark:text-red-400'
    };
  }

  // COM port sort key: "COM12" → 12
  function comPortNum(port) {
    const m = (port ?? '').match(/(\d+)$/);
    return m ? parseInt(m[1]) : 9999;
  }

  // ── Merged & sorted rows ──────────────────────────────────────────────────
  let rows = $derived.by(() => {
    const cardMap  = Object.fromEntries(simCards.map(c => [c.id, c]));
    const statsMap = Object.fromEntries(simStats.map(s => [s.sim_id, s]));
    return simsInfo
      .map(info => {
        const card  = cardMap[info.sim_id]  ?? {};
        const stats = statsMap[info.sim_id] ?? { recv: 0, sent: 0 };
        return { info, card, stats };
      })
      .sort((a, b) => comPortNum(a.info.com_port) - comPortNum(b.info.com_port));
  });

  // ── Data fetching ─────────────────────────────────────────────────────────
  async function fetchData() {
    if (isFetching) return;
    isFetching = true;
    try {
      const [infoRes, cardsRes, statsRes] = await Promise.all([
        apiClient.getAllSimsInfo(),
        apiClient.getAllSimCards(),
        apiClient.getSimStats(),
      ]);
      simsInfo  = Array.isArray(infoRes)  ? infoRes  : (infoRes?.data  ?? []);
      simCards  = Array.isArray(cardsRes) ? cardsRes : (cardsRes?.data ?? []);
      simStats  = Array.isArray(statsRes) ? statsRes : (statsRes?.data ?? []);
    } catch (e) {
      error = e?.message ?? $t('err_load_sim');
    } finally {
      isFetching = false;
      loading = false;
    }
  }

  async function refreshAll() {
    await fetchData();
  }

  let pollTimer;
  onMount(async () => {
    await refreshAll();
    pollTimer = setInterval(fetchData, 4000);
  });

  onDestroy(() => clearInterval(pollTimer));

  // ── Selection ─────────────────────────────────────────────────────────────
  function getSelectedSimIds() {
    return rows
      .filter((r) => selected.has(r.info.com_port))
      .map((r) => r.info.sim_id)
      .filter((id) => !!id);
  }

  function getSingleSelectedSimId() {
    const ids = getSelectedSimIds();
    return ids.length === 1 ? ids[0] : null;
  }

  function handleRowSelection(event, port, index) {
    const isShift = !!(event?.shiftKey);
    const next = new Set(selected);

    if (
      isShift &&
      lastClickedRowIndex !== null &&
      lastClickedRowIndex >= 0 &&
      lastClickedRowIndex < rows.length
    ) {
      const start = Math.min(lastClickedRowIndex, index);
      const end = Math.max(lastClickedRowIndex, index);
      for (const row of rows.slice(start, end + 1)) {
        next.add(row.info.com_port);
      }
      selected = next;
      lastClickedRowIndex = index;
      return;
    }

    next.has(port) ? next.delete(port) : next.add(port);
    selected = next;
    lastClickedRowIndex = index;
  }

  function areAllRowsSelected() {
    return rows.length > 0 && rows.every(r => selected.has(r.info.com_port));
  }

  function toggleAll() {
    if (areAllRowsSelected()) {
      selected = new Set();
    } else {
      selected = new Set(rows.map(r => r.info.com_port));
    }
    lastClickedRowIndex = null;
  }

  // ── Logout ────────────────────────────────────────────────────────────────
  // uses logout() imported from auth store

  let { onNavigate = () => {}, onNavigateCall = () => {}, onNavigateSim = () => {}, onNavigatePlatform = () => {}, onNavigatePhoneNumber = () => {}, onNavigatePlatformStats = () => {}, onNavigateMoney = () => {}, onNavigateMms = () => {} } = $props();
</script>

<div class="flex flex-col h-dvh w-screen bg-gray-50 dark:bg-zinc-950 text-sm font-sans">

  <!-- ── Top bar ──────────────────────────────────────────────────────────── -->
  <header class="flex items-center justify-between px-6 py-3 bg-white dark:bg-zinc-900 border-b border-gray-200 dark:border-zinc-800 shadow-sm">
    <div class="flex items-center gap-3">
      <img src="/cow.png" alt="" class="w-6 h-6 object-contain" />
      <h1 class="text-base font-semibold text-gray-800 dark:text-gray-100">{$t('sim_dashboard_title')}</h1>
      {#if !loading}
        <span class="text-xs text-gray-400 dark:text-gray-500">
          v{APP_VERSION}
        </span>
      {/if}
    </div>

    <div class="flex items-center gap-2">
      <button
        onclick={() => onNavigatePhoneNumber()}
        class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium
               border border-gray-200 dark:border-zinc-700
               text-gray-600 dark:text-gray-300
               hover:bg-gray-50 dark:hover:bg-zinc-800 transition"
      >
        <Icon icon="carbon:phone-voice" class="w-4 h-4" />
        {$t('btn_phone_number')}
      </button>
      <button
        onclick={() => onNavigatePlatform()}
        class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium
               border border-gray-200 dark:border-zinc-700
               text-gray-600 dark:text-gray-300
               hover:bg-gray-50 dark:hover:bg-zinc-800 transition"
      >
        <Icon icon="carbon:cloud-upload" class="w-4 h-4" />
        {$t('btn_connect_platform')}
      </button>
      <button
        onclick={() => onNavigatePlatformStats()}
        class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium
               border border-gray-200 dark:border-zinc-700
               text-gray-600 dark:text-gray-300
               hover:bg-gray-50 dark:hover:bg-zinc-800 transition"
      >
        <Icon icon="carbon:chart-column" class="w-4 h-4" />
        {$t('btn_platform_stats')}
      </button>
      <button
        onclick={() => onNavigateMoney()}
        class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium
               border border-gray-200 dark:border-zinc-700
               text-gray-600 dark:text-gray-300
               hover:bg-gray-50 dark:hover:bg-zinc-800 transition"
      >
        <Icon icon="carbon:currency" class="w-4 h-4" />
        {$t('btn_money')}
      </button>
      <button
        onclick={() => onNavigateMms()}
        class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium
               border border-gray-200 dark:border-zinc-700
               text-gray-600 dark:text-gray-300
               hover:bg-gray-50 dark:hover:bg-zinc-800 transition"
      >
        <Icon icon="carbon:image" class="w-4 h-4" />
        {$t('btn_mms')}
      </button>
      <button
        onclick={() => onNavigate(getSingleSelectedSimId())}
        class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium
               border border-gray-200 dark:border-zinc-700
               text-gray-600 dark:text-gray-300
               hover:bg-gray-50 dark:hover:bg-zinc-800 transition"
      >
        <Icon icon="carbon:chat" class="w-4 h-4" />
        {$t('col_sms')}
      </button>
      <button
        onclick={() => onNavigateCall(getSingleSelectedSimId())}
        class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium
               border border-gray-200 dark:border-zinc-700
               text-gray-600 dark:text-gray-300
               hover:bg-gray-50 dark:hover:bg-zinc-800 transition"
      >
        <Icon icon="carbon:phone" class="w-4 h-4" />
        {$t('call_log')}
      </button>
      <button
        onclick={() => { loading = true; error = ''; fetchData(); }}
        class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium
               border border-gray-200 dark:border-zinc-700
               text-gray-600 dark:text-gray-300
               hover:bg-gray-50 dark:hover:bg-zinc-800 transition"
      >
        <Icon icon="carbon:refresh" class="w-4 h-4" />
        {$t('btn_refresh')}
      </button>
      <button
        onclick={() => { showOperatorColumn = !showOperatorColumn; }}
        class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium
               border border-gray-200 dark:border-zinc-700
               text-gray-600 dark:text-gray-300
               hover:bg-gray-50 dark:hover:bg-zinc-800 transition"
        title={showOperatorColumn ? $t('hide_operator_column') : $t('show_operator_column')}
      >
        <Icon icon={showOperatorColumn ? 'carbon:chevron-left' : 'carbon:chevron-right'} class="w-4 h-4" />
        {showOperatorColumn ? $t('hide_operator_column') : $t('show_operator_column')}
      </button>
    </div>
  </header>

  <!-- ── Table area ──────────────────────────────────────────────────────── -->
  <div class="flex-1 overflow-auto pb-10">
    {#if error}
      <div class="flex items-center justify-center h-full">
        <div class="text-center">
          <Icon icon="carbon:warning" class="w-10 h-10 text-red-400 mx-auto mb-2" />
          <p class="text-red-500 font-medium">{error}</p>
        </div>
      </div>
    {:else}
      <table
        class="w-full border-collapse table-fixed text-xs [&_th]:overflow-hidden [&_td]:overflow-hidden"
        style:min-width={showOperatorColumn ? '1450px' : '1330px'}
      >
        <colgroup>
          <col style="width: 48px" />
          <col style="width: 76px" />
          <col style="width: 84px" />
          <col style="width: 104px" />
          <col style="width: 116px" />
          <col style="width: 158px" />
          {#if showOperatorColumn}
            <col style="width: 120px" />
          {/if}
          <col style="width: 92px" />
          <col style="width: 92px" />
          <col style="width: 72px" />
          <col style="width: 184px" />
          <col style="width: 150px" />
          <col style="width: 142px" />
        </colgroup>
        <!-- ── Header ── -->
        <thead class="sticky top-0 z-10 bg-gray-100 dark:bg-zinc-800">
          <tr>
            <!-- # / checkbox -->
            <th class="w-12 px-3 py-2.5 text-center border-b border-gray-200 dark:border-zinc-700">
              {#if !loading && rows.length > 0}
                <input
                  type="checkbox"
                  checked={areAllRowsSelected()}
                  onchange={toggleAll}
                  class="rounded cursor-pointer accent-blue-500"
                />
              {:else}
                <span class="text-gray-400 font-semibold">#</span>
              {/if}
            </th>
            {#each [$t('col_com_port'),$t('col_module'),$t('col_signal'),$t('col_network_status'),$t('col_phone_number')] as col}
              <th class="px-3 py-2.5 text-left font-semibold text-gray-600 dark:text-gray-300
                         border-b border-gray-200 dark:border-zinc-700 whitespace-nowrap">
                {col}
              </th>
            {/each}
            {#if showOperatorColumn}
              <th class="px-3 py-2.5 text-left font-semibold text-gray-600 dark:text-gray-300
                         border-b border-gray-200 dark:border-zinc-700 whitespace-nowrap">
                {$t('col_operator')}
              </th>
            {/if}
            {#each [$t('col_country'),$t('col_platform'),$t('col_sms'),'ICCID','IMSI','IMEI'] as col}
              <th class="px-3 py-2.5 text-left font-semibold text-gray-600 dark:text-gray-300
                         border-b border-gray-200 dark:border-zinc-700 whitespace-nowrap">
                {col}
              </th>
            {/each}
          </tr>
        </thead>

        <!-- ── Body ── -->
        <tbody>
          {#if loading}
            <!-- Skeleton rows -->
            {#each Array(8) as _, i}
              <tr class="border-b border-gray-100 dark:border-zinc-800 animate-pulse">
                <td class="px-3 py-2.5 text-center">
                  <div class="w-5 h-5 bg-gray-200 dark:bg-zinc-700 rounded mx-auto"></div>
                </td>
                {#each Array(visibleDataColumnCount) as _}
                  <td class="px-3 py-2.5">
                    <div class="h-3 bg-gray-200 dark:bg-zinc-700 rounded w-3/4"></div>
                  </td>
                {/each}
              </tr>
            {/each}
          {:else if rows.length === 0}
            <tr>
              <td colspan={visibleDataColumnCount + 1} class="px-6 py-12 text-center text-gray-400">
                <Icon icon="carbon:sim-card" class="w-8 h-8 mx-auto mb-2 opacity-40" />
                <p>{$t('no_sim_cards')}</p>
              </td>
            </tr>
          {:else}
            {#each rows as { info, card, stats }, i}
              {@const isSelected = selected.has(info.com_port)}
              {@const net = getNetStatus(info.network_registration)}
              {@const bars = signalBars(info.signal_quality?.rssi)}
              {@const hasSim = info.has_sim !== false}
              {@const imsi = card.imsi || info.imsi || ''}
              {@const mccSource = imsi || info.operator_info?.operator_id || ''}
              <tr
                class="border-b border-gray-100 dark:border-zinc-800 cursor-pointer
                       transition-colors duration-100
                       {!hasSim ? 'opacity-60' : ''}
                       {isSelected
                         ? 'bg-blue-50 dark:bg-blue-900/20'
                         : 'hover:bg-gray-50 dark:hover:bg-zinc-800/50'}"
                onclick={(e) => handleRowSelection(e, info.com_port, i)}
                onmouseenter={() => hoveredRow = info.com_port}
                onmouseleave={() => hoveredRow = null}
              >
                <!-- # / checkbox -->
                <td class="px-3 py-2.5 text-center text-gray-400 select-none">
                  {#if isSelected || hoveredRow === info.com_port}
                    <input
                      type="checkbox"
                      checked={isSelected}
                      onchange={(e) => handleRowSelection(e, info.com_port, i)}
                       onclick={(e) => e.stopPropagation()}
                      class="rounded cursor-pointer accent-blue-500"
                    />
                  {:else}
                    <span class="text-gray-400">{i + 1}</span>
                  {/if}
                </td>

                <!-- COM Port -->
                <td class="px-3 py-2.5 font-mono text-gray-700 dark:text-gray-200 whitespace-nowrap">
                  {displayPort(info.com_port) || '—'}
                </td>

                <!-- Module name -->
                <td class="px-3 py-2.5 text-gray-700 dark:text-gray-200 whitespace-nowrap">
                  {#if info.available === false}
                    <span class="text-gray-400">—</span>
                  {:else}
                    {getModuleLabel(info.model_info?.model)}
                  {/if}
                </td>

                <!-- Signal -->
                <td class="px-3 py-2.5 whitespace-nowrap">
                  {#if hasSim}
                  <div class="flex items-center gap-1.5">
                    <!-- Signal bars -->
                    <div class="flex items-end gap-px h-4">
                      {#each [1,2,3,4] as b}
                        <div
                          class="w-1 rounded-sm transition-all {b <= bars
                            ? bars >= 3 ? 'bg-green-500' : bars === 2 ? 'bg-yellow-400' : 'bg-red-400'
                            : 'bg-gray-200 dark:bg-zinc-600'}"
                          style="height: {b * 25}%"
                        ></div>
                      {/each}
                    </div>
                    <span class="text-gray-600 dark:text-gray-300 text-xs">
                      {rssiToDbm(info.signal_quality?.rssi)}
                    </span>
                  </div>
                  {:else if info.available === false}
                    <span class="text-gray-400">—</span>
                  {:else}
                    <span class="text-gray-400">—</span>
                  {/if}
                </td>

                <!-- Network Status -->
                <td class="px-3 py-2.5 whitespace-nowrap font-medium">
                  {#if hasSim}
                    <span class="{net.cls}">{net.key ? $t(net.key, { n: net.codeN }) : '—'}</span>
                  {:else if info.sim_status}
                    {@const statusBadge = getSimStatusBadge(info.sim_status)}
                    <span class="inline-flex items-center px-1.5 py-0.5 rounded text-xs font-medium {statusBadge.cls}">
                      {statusBadge.label}
                    </span>
                  {:else if info.available === false}
                    <span class="inline-flex items-center px-1.5 py-0.5 rounded text-xs font-medium bg-red-100 dark:bg-red-900/30 text-red-700 dark:text-red-400">{$t('sim_removed')}</span>
                  {:else}
                    <span class="inline-flex items-center px-1.5 py-0.5 rounded text-xs font-medium bg-red-100 dark:bg-red-900/30 text-red-700 dark:text-red-400">{$t('sim_removed')}</span>
                  {/if}
                </td>

                <!-- Phone Number -->
                <td class="px-3 py-2.5 whitespace-nowrap">
                  <div class="flex items-center gap-2">
                    <span class="font-mono text-gray-700 dark:text-gray-200">
                      {#if info.available === false}
                        —
                      {:else}
                        {normalizePhoneForDisplay(card.phone_number ?? info.phone_number) || '—'}
                      {/if}
                    </span>
                  </div>
                </td>

                <!-- Operator -->
                {#if showOperatorColumn}
                  <td class="px-3 py-2.5 text-gray-700 dark:text-gray-200 whitespace-nowrap">
                    {#if info.available === false}
                      <span class="text-gray-400">—</span>
                    {:else}
                      {info.operator_info?.operator_name ?? '—'}
                    {/if}
                  </td>
                {/if}

                <!-- Country -->
                <td class="px-3 py-2.5 text-gray-700 dark:text-gray-200 whitespace-nowrap">
                  {#if info.available === false}
                    <span class="text-gray-400">—</span>
                  {:else}
                    {mccSource ? getMccCountry(mccSource, $lang) : '—'}
                  {/if}
                </td>

                <!-- Platform status -->
                <td class="px-3 py-2.5 whitespace-nowrap">
                  {#if info.available === false}
                    <span class="text-gray-400">—</span>
                  {:else if card.country_code && ['1', '5'].includes(String(info.network_registration?.status ?? '').trim())}
                    <span class="inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs font-medium bg-green-100 dark:bg-green-900/30 text-green-700 dark:text-green-400">
                      <svg class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7"/></svg>
                      {$t('platform_connected')}
                    </span>
                  {:else}
                    <span class="text-gray-400 dark:text-gray-500 text-xs">{$t('platform_not_connected')}</span>
                  {/if}
                </td>

                <!-- SMS recv / sent -->
                <td class="px-3 py-2.5 text-center text-gray-700 dark:text-gray-200 whitespace-nowrap font-mono">
                  {#if info.available === false}
                    <span class="text-gray-400">—</span>
                  {:else}
                    <span class="text-green-600 dark:text-green-400">{stats.recv ?? 0}</span><span class="text-gray-400 dark:text-gray-500">/</span><span class="text-blue-500 dark:text-blue-400">{stats.sent ?? 0}</span>
                  {/if}
                </td>

                <!-- ICCID (= sim_id / card.id) -->
                <td class="px-3 py-2.5 font-mono text-gray-500 dark:text-gray-400 whitespace-nowrap">
                  {#if info.available === false}
                    <span class="text-gray-400">—</span>
                  {:else}
                    {card.id ?? info.sim_id ?? '—'}
                  {/if}
                </td>

                <!-- IMSI -->
                <td class="px-3 py-2.5 font-mono text-gray-500 dark:text-gray-400 whitespace-nowrap">
                  {#if info.available === false}
                    <span class="text-gray-400">—</span>
                  {:else}
                    {imsi || '—'}
                  {/if}
                </td>

                <!-- IMEI -->
                <td class="px-3 py-2.5 font-mono text-gray-500 dark:text-gray-400 whitespace-nowrap">
                  {info.imei ?? '—'}
                </td>
              </tr>
            {/each}
          {/if}
        </tbody>
      </table>
    {/if}
  </div>
</div>

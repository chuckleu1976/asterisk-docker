<script>
  import { onMount } from 'svelte';
  import Icon from '@iconify/svelte';
  import { apiClient } from '../js/api.js';
  import { logout } from '../stores/auth.js';

  // ── State ──────────────────────────────────────────────────────────────────
  let simsInfo   = $state([]);   // from /api/sims/info  (live AT data)
  let simCards   = $state([]);   // from /api/sim-cards  (DB: ICCID/IMSI)
  let simStats   = $state([]);   // from /api/sims/stats (SMS counts)
  let loading    = $state(true);
  let error      = $state('');
  let selected   = $state(new Set());
  let hoveredRow = $state(null);

  // ── Network status map ────────────────────────────────────────────────────
  const netStatus = {
    0: { label: 'Not Registered', cls: 'text-gray-400' },
    1: { label: 'Home',           cls: 'text-green-500' },
    2: { label: 'Searching',      cls: 'text-yellow-400' },
    3: { label: 'Denied',         cls: 'text-red-500' },
    4: { label: 'Unknown',        cls: 'text-gray-400' },
    5: { label: 'Roaming',        cls: 'text-blue-400' },
  };

  function getNetStatus(reg) {
    if (!reg) return { label: '—', cls: 'text-gray-400' };
    const s = netStatus[reg.status] ?? { label: `Code ${reg.status}`, cls: 'text-gray-400' };
    return s;
  }

  // RSSI → dBm helper
  function rssiToDbm(rssi) {
    if (rssi == null) return '—';
    if (rssi === 99) return 'N/A';
    return `${-113 + rssi * 2} dBm`;
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
  onMount(async () => {
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
      error = e?.message ?? 'Failed to load SIM data';
    } finally {
      loading = false;
    }
  });

  // ── Selection ─────────────────────────────────────────────────────────────
  function toggleRow(id) {
    const next = new Set(selected);
    next.has(id) ? next.delete(id) : next.add(id);
    selected = next;
  }

  function toggleAll() {
    if (selected.size === rows.length) {
      selected = new Set();
    } else {
      selected = new Set(rows.map(r => r.info.sim_id));
    }
  }

  // ── Logout ────────────────────────────────────────────────────────────────
  // uses logout() imported from auth store
</script>

<div class="flex flex-col h-dvh w-screen bg-gray-50 dark:bg-zinc-950 text-sm font-sans">

  <!-- ── Top bar ──────────────────────────────────────────────────────────── -->
  <header class="flex items-center justify-between px-6 py-3 bg-white dark:bg-zinc-900 border-b border-gray-200 dark:border-zinc-800 shadow-sm">
    <div class="flex items-center gap-3">
      <Icon icon="carbon:sim-card" class="w-5 h-5 text-gray-500 dark:text-gray-400" />
      <h1 class="text-base font-semibold text-gray-800 dark:text-gray-100">SIM Dashboard</h1>
      {#if !loading}
        <span class="text-xs text-gray-400 dark:text-gray-500">
          {rows.length} SIM{rows.length !== 1 ? 's' : ''}
          {#if selected.size > 0}· {selected.size} selected{/if}
        </span>
      {/if}
    </div>

    <div class="flex items-center gap-2">
      <button
        onclick={() => { loading = true; error = ''; onMount; location.reload(); }}
        class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium
               border border-gray-200 dark:border-zinc-700
               text-gray-600 dark:text-gray-300
               hover:bg-gray-50 dark:hover:bg-zinc-800 transition"
      >
        <Icon icon="carbon:refresh" class="w-4 h-4" />
        Refresh
      </button>
      <button
        onclick={logout}
        class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium
               border border-gray-200 dark:border-zinc-700
               text-gray-600 dark:text-gray-300
               hover:bg-gray-50 dark:hover:bg-zinc-800 transition"
      >
        <Icon icon="carbon:logout" class="w-4 h-4" />
        Logout
      </button>
    </div>
  </header>

  <!-- ── Table area ──────────────────────────────────────────────────────── -->
  <div class="flex-1 overflow-auto">
    {#if error}
      <div class="flex items-center justify-center h-full">
        <div class="text-center">
          <Icon icon="carbon:warning" class="w-10 h-10 text-red-400 mx-auto mb-2" />
          <p class="text-red-500 font-medium">{error}</p>
        </div>
      </div>
    {:else}
      <table class="w-full border-collapse text-xs">
        <!-- ── Header ── -->
        <thead class="sticky top-0 z-10 bg-gray-100 dark:bg-zinc-800">
          <tr>
            <!-- # / checkbox -->
            <th class="w-12 px-3 py-2.5 text-center border-b border-gray-200 dark:border-zinc-700">
              {#if !loading && rows.length > 0}
                <input
                  type="checkbox"
                  checked={selected.size === rows.length && rows.length > 0}
                  onchange={toggleAll}
                  class="rounded cursor-pointer accent-blue-500"
                />
              {:else}
                <span class="text-gray-400 font-semibold">#</span>
              {/if}
            </th>
            {#each ['COM Port','Module','Signal','Network Status','Phone Number','Operator','SMS Recv','SMS Sent','IMSI','ICCID','IMEI'] as col}
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
                {#each Array(11) as _}
                  <td class="px-3 py-2.5">
                    <div class="h-3 bg-gray-200 dark:bg-zinc-700 rounded w-3/4"></div>
                  </td>
                {/each}
              </tr>
            {/each}
          {:else if rows.length === 0}
            <tr>
              <td colspan="12" class="px-6 py-12 text-center text-gray-400">
                <Icon icon="carbon:sim-card" class="w-8 h-8 mx-auto mb-2 opacity-40" />
                <p>No SIM cards found</p>
              </td>
            </tr>
          {:else}
            {#each rows as { info, card, stats }, i}
              {@const isSelected = selected.has(info.sim_id)}
              {@const net = getNetStatus(info.network_registration)}
              {@const bars = signalBars(info.signal_quality?.rssi)}
              {@const hasSim = info.has_sim !== false}
              <tr
                class="border-b border-gray-100 dark:border-zinc-800 cursor-pointer
                       transition-colors duration-100
                       {!hasSim ? 'opacity-60' : ''}
                       {isSelected
                         ? 'bg-blue-50 dark:bg-blue-900/20'
                         : 'hover:bg-gray-50 dark:hover:bg-zinc-800/50'}"
                onclick={() => toggleRow(info.sim_id)}
                onmouseenter={() => hoveredRow = info.sim_id}
                onmouseleave={() => hoveredRow = null}
              >
                <!-- # / checkbox -->
                <td class="px-3 py-2.5 text-center text-gray-400 select-none">
                  {#if isSelected || hoveredRow === info.sim_id}
                    <input
                      type="checkbox"
                      checked={isSelected}
                      onchange={() => toggleRow(info.sim_id)}
                       onclick={(e) => e.stopPropagation()}
                      class="rounded cursor-pointer accent-blue-500"
                    />
                  {:else}
                    <span class="text-gray-400">{i + 1}</span>
                  {/if}
                </td>

                <!-- COM Port -->
                <td class="px-3 py-2.5 font-mono text-gray-700 dark:text-gray-200 whitespace-nowrap">
                  {info.com_port ?? '—'}
                </td>

                <!-- Module name -->
                <td class="px-3 py-2.5 text-gray-700 dark:text-gray-200 whitespace-nowrap">
                  {info.model_info?.model ?? '—'}
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
                  {:else}
                    <span class="text-gray-400">—</span>
                  {/if}
                </td>

                <!-- Network Status -->
                <td class="px-3 py-2.5 whitespace-nowrap font-medium">
                  {#if hasSim}
                    <span class="{net.cls}">{net.label}</span>
                  {:else}
                    <span class="inline-flex items-center px-1.5 py-0.5 rounded text-xs bg-gray-100 dark:bg-zinc-700 text-gray-500 dark:text-gray-400">No SIM</span>
                  {/if}
                </td>

                <!-- Phone Number -->
                <td class="px-3 py-2.5 font-mono text-gray-700 dark:text-gray-200 whitespace-nowrap">
                  {card.phone_number ?? info.phone_number ?? '—'}
                </td>

                <!-- Operator -->
                <td class="px-3 py-2.5 text-gray-700 dark:text-gray-200 whitespace-nowrap">
                  {info.operator_info?.operator_name ?? '—'}
                </td>

                <!-- SMS Recv -->
                <td class="px-3 py-2.5 text-center text-gray-700 dark:text-gray-200">
                  {stats.recv ?? 0}
                </td>

                <!-- SMS Sent -->
                <td class="px-3 py-2.5 text-center text-gray-700 dark:text-gray-200">
                  {stats.sent ?? 0}
                </td>

                <!-- IMSI -->
                <td class="px-3 py-2.5 font-mono text-gray-500 dark:text-gray-400 whitespace-nowrap">
                  {card.imsi ?? '—'}
                </td>

                <!-- ICCID (= sim_id / card.id) -->
                <td class="px-3 py-2.5 font-mono text-gray-500 dark:text-gray-400 whitespace-nowrap">
                  {card.id ?? info.sim_id ?? '—'}
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

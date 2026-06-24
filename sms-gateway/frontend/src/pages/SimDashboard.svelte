<script>
  import { onMount, onDestroy } from 'svelte';
  import Icon from '@iconify/svelte';
  import { apiClient } from '../js/api.js';
  import { logout } from '../stores/auth.js';
  import { t, lang } from '../js/i18n.js';

  // ── MCC → Country map ─────────────────────────────────────────────────────
  const mccMap = {
    '460': { en: 'China',        zh: '中国' },
    '461': { en: 'China',        zh: '中国' },
    '466': { en: 'Taiwan',       zh: '台湾' },
    '454': { en: 'Hong Kong',    zh: '香港' },
    '455': { en: 'Macau',        zh: '澳门' },
    '440': { en: 'Japan',        zh: '日本' },
    '441': { en: 'Japan',        zh: '日本' },
    '450': { en: 'Korea',        zh: '韩国' },
    '452': { en: 'Vietnam',      zh: '越南' },
    '456': { en: 'Cambodia',     zh: '柬埔寨' },
    '457': { en: 'Laos',         zh: '老挝' },
    '502': { en: 'Malaysia',     zh: '马来西亚' },
    '510': { en: 'Indonesia',    zh: '印度尼西亚' },
    '515': { en: 'Philippines',  zh: '菲律宾' },
    '520': { en: 'Thailand',     zh: '泰国' },
    '525': { en: 'Singapore',    zh: '新加坡' },
    '404': { en: 'India',        zh: '印度' },
    '405': { en: 'India',        zh: '印度' },
    '406': { en: 'India',        zh: '印度' },
    '410': { en: 'Pakistan',     zh: '巴基斯坦' },
    '411': { en: 'Pakistan',     zh: '巴基斯坦' },
    '470': { en: 'Bangladesh',   zh: '孟加拉国' },
    '414': { en: 'Myanmar',      zh: '缅甸' },
    '413': { en: 'Sri Lanka',    zh: '斯里兰卡' },
    '429': { en: 'Nepal',        zh: '尼泊尔' },
    '428': { en: 'Mongolia',     zh: '蒙古' },
    '401': { en: 'Kazakhstan',   zh: '哈萨克斯坦' },
    '434': { en: 'Uzbekistan',   zh: '乌兹别克斯坦' },
    '432': { en: 'Iran',         zh: '伊朗' },
    '418': { en: 'Iraq',         zh: '伊拉克' },
    '425': { en: 'Israel',       zh: '以色列' },
    '416': { en: 'Jordan',       zh: '约旦' },
    '415': { en: 'Lebanon',      zh: '黎巴嫩' },
    '419': { en: 'Kuwait',       zh: '科威特' },
    '426': { en: 'Bahrain',      zh: '巴林' },
    '427': { en: 'Qatar',        zh: '卡塔尔' },
    '420': { en: 'Saudi Arabia', zh: '沙特阿拉伯' },
    '424': { en: 'UAE',          zh: '阿联酋' },
    '430': { en: 'UAE',          zh: '阿联酋' },
    '431': { en: 'UAE',          zh: '阿联酋' },
    '286': { en: 'Turkey',       zh: '土耳其' },
    '250': { en: 'Russia',       zh: '俄罗斯' },
    '255': { en: 'Ukraine',      zh: '乌克兰' },
    '202': { en: 'Greece',       zh: '希腊' },
    '204': { en: 'Netherlands',  zh: '荷兰' },
    '206': { en: 'Belgium',      zh: '比利时' },
    '208': { en: 'France',       zh: '法国' },
    '214': { en: 'Spain',        zh: '西班牙' },
    '216': { en: 'Hungary',      zh: '匈牙利' },
    '218': { en: 'Bosnia',       zh: '波斯尼亚' },
    '219': { en: 'Croatia',      zh: '克罗地亚' },
    '220': { en: 'Serbia',       zh: '塞尔维亚' },
    '222': { en: 'Italy',        zh: '意大利' },
    '226': { en: 'Romania',      zh: '罗马尼亚' },
    '228': { en: 'Switzerland',  zh: '瑞士' },
    '230': { en: 'Czechia',      zh: '捷克' },
    '231': { en: 'Slovakia',     zh: '斯洛伐克' },
    '232': { en: 'Austria',      zh: '奥地利' },
    '234': { en: 'UK',           zh: '英国' },
    '235': { en: 'UK',           zh: '英国' },
    '238': { en: 'Denmark',      zh: '丹麦' },
    '240': { en: 'Sweden',       zh: '瑞典' },
    '242': { en: 'Norway',       zh: '挪威' },
    '244': { en: 'Finland',      zh: '芬兰' },
    '246': { en: 'Lithuania',    zh: '立陶宛' },
    '247': { en: 'Latvia',       zh: '拉脱维亚' },
    '248': { en: 'Estonia',      zh: '爱沙尼亚' },
    '260': { en: 'Poland',       zh: '波兰' },
    '262': { en: 'Germany',      zh: '德国' },
    '268': { en: 'Portugal',     zh: '葡萄牙' },
    '272': { en: 'Ireland',      zh: '爱尔兰' },
    '284': { en: 'Bulgaria',     zh: '保加利亚' },
    '293': { en: 'Slovenia',     zh: '斯洛文尼亚' },
    '302': { en: 'Canada',       zh: '加拿大' },
    '310': { en: 'USA',          zh: '美国' },
    '311': { en: 'USA',          zh: '美国' },
    '312': { en: 'USA',          zh: '美国' },
    '313': { en: 'USA',          zh: '美国' },
    '314': { en: 'USA',          zh: '美国' },
    '315': { en: 'USA',          zh: '美国' },
    '316': { en: 'USA',          zh: '美国' },
    '334': { en: 'Mexico',       zh: '墨西哥' },
    '505': { en: 'Australia',    zh: '澳大利亚' },
    '530': { en: 'New Zealand',  zh: '新西兰' },
    '602': { en: 'Egypt',        zh: '埃及' },
    '603': { en: 'Algeria',      zh: '阿尔及利亚' },
    '604': { en: 'Morocco',      zh: '摩洛哥' },
    '605': { en: 'Tunisia',      zh: '突尼斯' },
    '620': { en: 'Ghana',        zh: '加纳' },
    '621': { en: 'Nigeria',      zh: '尼日利亚' },
    '636': { en: 'Ethiopia',     zh: '埃塞俄比亚' },
    '639': { en: 'Kenya',        zh: '肯尼亚' },
    '640': { en: 'Tanzania',     zh: '坦桑尼亚' },
    '655': { en: 'South Africa', zh: '南非' },
    '722': { en: 'Argentina',    zh: '阿根廷' },
    '724': { en: 'Brazil',       zh: '巴西' },
    '725': { en: 'Brazil',       zh: '巴西' },
    '730': { en: 'Chile',        zh: '智利' },
    '732': { en: 'Colombia',     zh: '哥伦比亚' },
    '733': { en: 'Colombia',     zh: '哥伦比亚' },
  };

  function getMccCountry(imsi, langVal) {
    if (!imsi || String(imsi).length < 3) return '—';
    const mcc = String(imsi).slice(0, 3);
    const entry = mccMap[mcc];
    if (!entry) return mcc;
    return entry[langVal] ?? entry.en;
  }

  // ── State ──────────────────────────────────────────────────────────────────
  let simsInfo   = $state([]);   // from /api/sims/info  (live AT data)
  let simCards   = $state([]);   // from /api/sim-cards  (DB: ICCID/IMSI)
  let simStats   = $state([]);   // from /api/sims/stats (SMS counts)
  let loading    = $state(true);
  let error      = $state('');
  let selected   = $state(new Set());
  let hoveredRow = $state(null);

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
      loading = false;
    }
  }

  let pollTimer;
  onMount(async () => {
    await fetchData();
    pollTimer = setInterval(fetchData, 4000);
  });

  onDestroy(() => clearInterval(pollTimer));

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

  let { onNavigate = () => {}, onNavigateCall = () => {}, onNavigateSim = () => {} } = $props();
</script>

<div class="flex flex-col h-dvh w-screen bg-gray-50 dark:bg-zinc-950 text-sm font-sans">

  <!-- ── Top bar ──────────────────────────────────────────────────────────── -->
  <header class="flex items-center justify-between px-6 py-3 bg-white dark:bg-zinc-900 border-b border-gray-200 dark:border-zinc-800 shadow-sm">
    <div class="flex items-center gap-3">
      <Icon icon="carbon:sim-card" class="w-5 h-5 text-gray-500 dark:text-gray-400" />
      <h1 class="text-base font-semibold text-gray-800 dark:text-gray-100">{$t('sim_dashboard_title')}</h1>
      {#if !loading}
        <span class="text-xs text-gray-400 dark:text-gray-500">
          {rows.length === 1 ? $t('sim_count', { n: rows.length }) : $t('sim_count_plural', { n: rows.length })}
          {#if selected.size > 0}· {$t('selected_count', { n: selected.size })}{/if}
        </span>
      {/if}
    </div>

    <div class="flex items-center gap-2">
      <button
        onclick={() => onNavigateSim(selected.size === 1 ? [...selected][0] : null)}
        class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium
               border border-gray-200 dark:border-zinc-700
               text-gray-600 dark:text-gray-300
               hover:bg-gray-50 dark:hover:bg-zinc-800 transition"
      >
        <Icon icon="carbon:sim-card" class="w-4 h-4" />
        {$t('sim_cards')}
      </button>
      <button
        onclick={() => onNavigateCall(selected.size === 1 ? [...selected][0] : null)}
        class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium
               border border-gray-200 dark:border-zinc-700
               text-gray-600 dark:text-gray-300
               hover:bg-gray-50 dark:hover:bg-zinc-800 transition"
      >
        <Icon icon="carbon:phone" class="w-4 h-4" />
        {$t('call_log')}
      </button>
      <button
        onclick={() => onNavigate(selected.size === 1 ? [...selected][0] : null)}
        class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium
               border border-gray-200 dark:border-zinc-700
               text-gray-600 dark:text-gray-300
               hover:bg-gray-50 dark:hover:bg-zinc-800 transition"
      >
        <Icon icon="carbon:chat" class="w-4 h-4" />
        {$t('btn_messages')}
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
        onclick={logout}
        class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium
               border border-gray-200 dark:border-zinc-700
               text-gray-600 dark:text-gray-300
               hover:bg-gray-50 dark:hover:bg-zinc-800 transition"
      >
        <Icon icon="carbon:logout" class="w-4 h-4" />
        {$t('btn_logout')}
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
            {#each [$t('col_com_port'),$t('col_module'),$t('col_network_status'),$t('col_phone_number'),$t('col_operator'),$t('col_country'),$t('col_sms'),'IMSI','ICCID','IMEI'] as col}
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
                {#each Array(10) as _}
                  <td class="px-3 py-2.5">
                    <div class="h-3 bg-gray-200 dark:bg-zinc-700 rounded w-3/4"></div>
                  </td>
                {/each}
              </tr>
            {/each}
          {:else if rows.length === 0}
            <tr>
              <td colspan="11" class="px-6 py-12 text-center text-gray-400">
                <Icon icon="carbon:sim-card" class="w-8 h-8 mx-auto mb-2 opacity-40" />
                <p>{$t('no_sim_cards')}</p>
              </td>
            </tr>
          {:else}
            {#each rows as { info, card, stats }, i}
              {@const isSelected = selected.has(info.sim_id)}
              {@const net = getNetStatus(info.network_registration)}
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
                  {#if info.available === false}
                    <span class="inline-flex items-center px-1.5 py-0.5 rounded text-xs bg-red-100 dark:bg-red-900/30 text-red-600 dark:text-red-400">
                      {$t('unavailable')}
                    </span>
                  {:else}
                    {info.model_info?.model ?? '—'}
                  {/if}
                </td>

                
                <!-- Network Status -->
                <td class="px-3 py-2.5 whitespace-nowrap font-medium">
                  {#if hasSim}
                    <span class="{net.cls}">{net.key ? $t(net.key, { n: net.codeN }) : '—'}</span>
                  {:else}
                    <span class="inline-flex items-center px-1.5 py-0.5 rounded text-xs bg-gray-100 dark:bg-zinc-700 text-gray-500 dark:text-gray-400">{$t('no_sim')}</span>
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

                <!-- Country -->
                <td class="px-3 py-2.5 text-gray-700 dark:text-gray-200 whitespace-nowrap">
                  {getMccCountry(card.imsi ?? info.imsi, $lang)}
                </td>

                <!-- SMS recv / sent -->
                <td class="px-3 py-2.5 text-center text-gray-700 dark:text-gray-200 whitespace-nowrap font-mono">
                  <span class="text-green-600 dark:text-green-400">{stats.recv ?? 0}</span><span class="text-gray-400 dark:text-gray-500">/</span><span class="text-blue-500 dark:text-blue-400">{stats.sent ?? 0}</span>
                </td>

                <!-- IMSI -->
                <td class="px-3 py-2.5 font-mono text-gray-500 dark:text-gray-400 whitespace-nowrap">
                  {card.imsi ?? info.imsi ?? '—'}
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

<script>
  import { onMount, onDestroy, tick } from 'svelte';
  import Icon from '@iconify/svelte';
  import { apiClient } from '../js/api.js';
  import { t } from '../js/i18n.js';
  import { displayPort } from '../js/modem.js';

  let { onBack = () => {}, onNavigateEsim = () => {} } = $props();

  // ── State ──────────────────────────────────────────────────────────────────
  let activeTab = $state('barcode'); // 'barcode' | 'import'
  let sims = $state([]);
  let simCards = $state([]);
  let loading = $state(true);
  let error = $state('');

  // Import tab
  let fileInput = $state(null);
  let importText = $state('');
  let importPreview = $state([]);
  let importRunning = $state(false);

  // Barcode tab
  let barcodeIccid = $state('');
  let barcodeMsisdn = $state('');
  let barcodeScans = $state([]);
  let barcodeLoading = $state(false);
  let barcodeImporting = $state(false);
  let iccidInput = $state(null);
  let msisdnInput = $state(null);

  // Status polling
  let statusTask = $state(null);
  let taskStatus = $state({ running: false, task_type: '', total: 0, done: 0, current: '', errors: [], results: [] });

  // ── Data fetching ──────────────────────────────────────────────────────────
  async function fetchData() {
    loading = true;
    error = '';
    try {
      const [infoRes, cardsRes] = await Promise.all([
        apiClient.getAllSimsInfo(),
        apiClient.getAllSimCards(),
      ]);
      const infos = Array.isArray(infoRes) ? infoRes : (infoRes?.data ?? []);
      const cards = Array.isArray(cardsRes) ? cardsRes : (cardsRes?.data ?? []);
      simCards = cards;

      const cardMap = Object.fromEntries(cards.map(c => [c.id, c]));
      sims = infos
        .filter(info => info.has_sim !== false && info.sim_id && !info.sim_id.startsWith('fallback_sim_'))
        .map(info => {
          const card = cardMap[info.sim_id] ?? {};
          return {
            ...info,
            phone_number: card.phone_number ?? info.phone_number ?? null,
          };
        })
        .sort((a, b) => comIndex(a.com_port) - comIndex(b.com_port));
    } catch (e) {
      error = e?.message ?? $t('err_load_sim');
    } finally {
      loading = false;
    }
  }

  function comIndex(port) {
    const n = parseInt((port ?? '').replace(/^COM/i, ''), 10);
    return isNaN(n) ? Infinity : n;
  }

  function normalizeMsisdn(raw) {
    const text = String(raw ?? '').trim();
    if (!text) return '';

    if (text.startsWith('+')) {
      const digits = text.slice(1).replace(/\D/g, '').replace(/^0+/, '');
      return digits ? `+${digits}` : '';
    }

    return text.replace(/\D/g, '').replace(/^0+/, '');
  }

  // ── Status polling ─────────────────────────────────────────────────────────
  async function pollStatus() {
    try {
      const res = await apiClient.getPhoneNumberStatus();
      taskStatus = res?.data ?? res ?? taskStatus;
      const running = taskStatus.running;

      if (activeTab === 'import') importRunning = running && taskStatus.task_type === 'import';

      if (running) {
        statusTask = setTimeout(pollStatus, 1500);
      } else {
        // Refresh SIM list when task finishes
        fetchData();
      }
    } catch (e) {
      statusTask = setTimeout(pollStatus, 3000);
    }
  }

  function startPolling() {
    if (statusTask) clearTimeout(statusTask);
    pollStatus();
  }

  async function focusBarcodeIccidInput() {
    await tick();
    iccidInput?.focus();
    iccidInput?.select?.();
  }

  async function setActiveTab(tabId) {
    activeTab = tabId;
    error = '';
    if (tabId === 'barcode') {
      fetchBarcodeScans();
      await focusBarcodeIccidInput();
    }
  }

  $effect(() => {
    if (activeTab === 'barcode' && !loading) {
      // Re-apply focus when the barcode pane becomes rendered after async loads.
      void focusBarcodeIccidInput();
    }
  });

  // ── Import tab ─────────────────────────────────────────────────────────────
  function parseImportText(text) {
    const lines = text.split(/\r?\n/).filter(l => l.trim());
    return lines.map(line => {
      const parts = line.split(',').map(s => s.trim());
      return {
        iccid: parts[0] ?? '',
        msisdn: normalizeMsisdn(parts[1] ?? ''),
      };
    }).filter(e => e.iccid && e.msisdn);
  }

  function updatePreview() {
    importPreview = parseImportText(importText).map(entry => {
      const matched = sims.find(s => s.sim_id === entry.iccid);
      return {
        ...entry,
        com_port: matched?.com_port ?? '-',
        status: matched ? $t('phone_import_status_ready') : '未匹配',
      };
    });
  }

  function handleFileSelect(e) {
    const file = e.target.files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = (ev) => {
      importText = ev.target.result;
      updatePreview();
    };
    reader.readAsText(file);
  }

  function handleDrop(e) {
    e.preventDefault();
    const file = e.dataTransfer.files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = (ev) => {
      importText = ev.target.result;
      updatePreview();
    };
    reader.readAsText(file);
  }

  async function startImport() {
    const entries = parseImportText(importText);
    if (entries.length === 0) return;
    importRunning = true;
    try {
      await apiClient.importPhoneNumbers(entries);
      startPolling();
    } catch (e) {
      error = e?.data?.error ?? e?.message ?? $t('err_upload_failed');
      importRunning = false;
    }
  }

  // ── Barcode tab ────────────────────────────────────────────────────────────
  function validateBarcodeIccid(value) {
    const digits = String(value ?? '').replace(/\D/g, '');
    return digits.startsWith('8944') && digits.length === 20;
  }

  function validateBarcodeMsisdn(value) {
    const digits = String(value ?? '').replace(/\D/g, '');
    if (digits.length !== 11) return false;
    return ['077', '071', '073', '074', '075', '078', '079'].some(p => digits.startsWith(p));
  }

  function sanitizeBarcodeInput(value, maxLen) {
    return String(value ?? '').replace(/\D/g, '').slice(0, maxLen);
  }

  function onIccidInput(e) {
    barcodeIccid = sanitizeBarcodeInput(e.target.value, 20);
    e.target.value = barcodeIccid;
    if (validateBarcodeIccid(barcodeIccid)) {
      msisdnInput?.focus();
    }
  }

  function onMsisdnInput(e) {
    barcodeMsisdn = sanitizeBarcodeInput(e.target.value, 11);
    e.target.value = barcodeMsisdn;
    if (validateBarcodeMsisdn(barcodeMsisdn)) {
      submitBarcodeScan();
    }
  }

  async function submitBarcodeScan() {
    if (!validateBarcodeIccid(barcodeIccid) || !validateBarcodeMsisdn(barcodeMsisdn)) {
      return;
    }
    barcodeLoading = true;
    error = '';
    try {
      await apiClient.barcodeScan(barcodeIccid, barcodeMsisdn);
      barcodeIccid = '';
      barcodeMsisdn = '';
      iccidInput?.focus();
      await fetchBarcodeScans();
    } catch (e) {
      error = e?.data?.error ?? e?.message ?? $t('barcode_scan_error');
    } finally {
      barcodeLoading = false;
    }
  }

  async function fetchBarcodeScans() {
    try {
      const res = await apiClient.getBarcodeScans();
      barcodeScans = Array.isArray(res) ? res : (res?.data ?? []);
    } catch (e) {
      console.error('Failed to load barcode scans:', e);
    }
  }

  async function clearBarcodeInputs() {
    barcodeIccid = '';
    barcodeMsisdn = '';
    await focusBarcodeIccidInput();
  }

  async function importBarcodeScans() {
    if (barcodeScans.length === 0) return;
    barcodeImporting = true;
    error = '';
    try {
      const res = await apiClient.importBarcodeScans();
      await fetchBarcodeScans();
      startPolling();
    } catch (e) {
      error = e?.data?.error ?? e?.message ?? $t('barcode_import_error');
      await fetchBarcodeScans();
    } finally {
      barcodeImporting = false;
    }
  }

  // ── Helpers ────────────────────────────────────────────────────────────────
  function formatPhone(p) {
    if (!p) return $t('phone_result_empty');
    return p;
  }

  function statusClass(status) {
    switch (String(status ?? '').toLowerCase()) {
      case 'success': return 'text-green-600 dark:text-green-400';
      case 'failed': return 'text-red-600 dark:text-red-400';
      case 'skipped': return 'text-yellow-600 dark:text-yellow-400';
      default: return 'text-gray-500 dark:text-gray-400';
    }
  }

  onMount(async () => {
    await fetchData();
    if (activeTab === 'barcode') {
      await fetchBarcodeScans();
      await focusBarcodeIccidInput();
    }
  });
  onDestroy(() => {
    if (statusTask) clearTimeout(statusTask);
  });
</script>

<div class="flex flex-col h-dvh w-screen bg-gray-50 dark:bg-zinc-950 text-sm font-sans">
  <!-- ── Header ─────────────────────────────────────────────────────────────── -->
  <header class="flex items-center gap-3 px-4 py-3 bg-white dark:bg-zinc-900 border-b border-gray-200 dark:border-zinc-800 shadow-sm">
    <button
      onclick={onBack}
      class="inline-flex items-center gap-1.5 h-9 px-2.5 rounded-full bg-blue-600 text-white
             shadow-sm shadow-blue-600/30 hover:bg-blue-700 transition"
      aria-label="Back"
    >
      <Icon icon="carbon:arrow-left" class="w-4 h-4" />
      <span class="text-xs font-semibold">{$t('btn_back')}</span>
    </button>
    <Icon icon="carbon:phone-voice" class="w-5 h-5 text-gray-500 dark:text-gray-400" />
    <h1 class="text-base font-semibold text-gray-800 dark:text-gray-100">{$t('phone_number_title')}</h1>
  </header>

  <!-- ── Tabs ───────────────────────────────────────────────────────────────── -->
  <div class="px-4 pt-4 bg-white dark:bg-zinc-900 border-b border-gray-200 dark:border-zinc-800">
    <div class="flex gap-1 overflow-x-auto">
      {#each [
        { id: 'barcode', label: $t('phone_tab_barcode'), icon: 'carbon:scan' },
        { id: 'import', label: $t('phone_tab_import'), icon: 'carbon:document-import' },
      ] as tab}
        <button
          onclick={() => setActiveTab(tab.id)}
          class="flex items-center gap-1.5 px-4 py-2 text-sm font-medium whitespace-nowrap border-b-2 transition
                 {activeTab === tab.id
                   ? 'border-blue-500 text-blue-600 dark:text-blue-400'
                   : 'border-transparent text-gray-600 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200'}"
        >
          <Icon icon={tab.icon} class="w-4 h-4" />
          {tab.label}
        </button>
      {/each}
      <button
        onclick={() => onNavigateEsim()}
        class="flex items-center gap-1.5 px-4 py-2 text-sm font-medium whitespace-nowrap border-b-2 border-transparent
               text-gray-600 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200 transition"
      >
        <Icon icon="carbon:sim-card" class="w-4 h-4" />
        {$t('esim_title')}
      </button>
    </div>
  </div>

  <!-- ── Content ────────────────────────────────────────────────────────────── -->
  <div class="flex-1 overflow-auto p-4 sm:p-6">
    {#if loading}
      <div class="animate-pulse space-y-4 max-w-4xl mx-auto">
        <div class="h-8 bg-gray-200 dark:bg-zinc-800 rounded w-1/3"></div>
        <div class="h-32 bg-gray-200 dark:bg-zinc-800 rounded"></div>
      </div>
    {:else}
      <div class="max-w-5xl mx-auto space-y-4">
        {#if error}
          <div class="p-3 rounded-lg bg-red-50 dark:bg-red-900/20 text-red-700 dark:text-red-300 text-sm flex items-start gap-2">
            <Icon icon="carbon:warning" class="w-5 h-5 flex-shrink-0" />
            <span>{error}</span>
          </div>
        {/if}

        <!-- Import -->
        {#if activeTab === 'import'}
          <div class="bg-white dark:bg-zinc-900 rounded-xl border border-gray-200 dark:border-zinc-800 p-4 sm:p-6 shadow-sm space-y-4">
            <p class="text-gray-600 dark:text-gray-400 text-sm">{$t('phone_import_hint')}</p>

            <div
              role="button"
              tabindex="0"
              ondragover={(e) => e.preventDefault()}
              ondrop={handleDrop}
              onclick={() => fileInput?.click()}
              onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') fileInput?.click(); }}
              class="border-2 border-dashed border-gray-300 dark:border-zinc-700 rounded-xl p-8 text-center cursor-pointer hover:border-blue-400 dark:hover:border-blue-500 transition"
            >
              <Icon icon="carbon:cloud-upload" class="w-8 h-8 mx-auto text-gray-400 dark:text-gray-500 mb-2" />
              <p class="text-gray-600 dark:text-gray-400">{$t('phone_import_file_hint')}</p>
              <input bind:this={fileInput} type="file" accept=".txt,.csv" class="hidden" onchange={handleFileSelect} />
            </div>

            <textarea
              bind:value={importText}
              oninput={updatePreview}
              placeholder="89441000304692509546,07770065802&#10;89441000304698110844,07770065803"
              rows="6"
              class="w-full rounded-lg border border-gray-300 dark:border-zinc-700 bg-white dark:bg-zinc-900 px-3 py-2 text-sm font-mono focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none"
            ></textarea>

            {#if importPreview.length > 0}
              <div>
                <h3 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">{$t('phone_import_preview')}</h3>
                <div class="overflow-x-auto rounded-lg border border-gray-200 dark:border-zinc-800">
                  <table class="min-w-full text-sm">
                    <thead class="bg-gray-50 dark:bg-zinc-800 text-gray-600 dark:text-gray-400">
                      <tr>
                        <th class="px-3 py-2 text-left font-medium">{$t('phone_import_col_iccid')}</th>
                        <th class="px-3 py-2 text-left font-medium">{$t('phone_import_col_msisdn')}</th>
                        <th class="px-3 py-2 text-left font-medium">{$t('phone_import_col_com')}</th>
                        <th class="px-3 py-2 text-left font-medium">{$t('phone_import_col_status')}</th>
                      </tr>
                    </thead>
                    <tbody class="divide-y divide-gray-200 dark:divide-zinc-800">
                      {#each importPreview as row}
                        <tr class="hover:bg-gray-50 dark:hover:bg-zinc-800/50">
                          <td class="px-3 py-2 font-mono text-xs">{row.iccid}</td>
                          <td class="px-3 py-2 font-mono text-xs">{row.msisdn}</td>
                          <td class="px-3 py-2">{displayPort(row.com_port) || '-'}</td>
                          <td class="px-3 py-2">{row.status}</td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                </div>
              </div>
            {/if}

            <div class="flex items-center justify-between">
              <div class="text-sm text-gray-500 dark:text-gray-400 space-y-1">
                {#if taskStatus.running && taskStatus.task_type === 'import'}
                  <div>{$t('import_progress', { done: taskStatus.done, total: taskStatus.total })}</div>
                {/if}
              </div>
              <div class="flex items-center gap-2">
                <button
                  onclick={startImport}
                  disabled={importRunning || importPreview.length === 0}
                  class="inline-flex items-center gap-2 px-4 py-2 rounded-lg text-sm font-medium text-white bg-blue-600 hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed transition"
                >
                  {#if importRunning}
                    <Icon icon="carbon:loading" class="w-4 h-4 animate-spin" />
                  {:else}
                    <Icon icon="carbon:document-import" class="w-4 h-4" />
                  {/if}
                  {$t('btn_start_import')}
                </button>
              </div>
            </div>
          </div>

        <!-- Barcode scan -->
        {:else if activeTab === 'barcode'}
          <div class="bg-white dark:bg-zinc-900 rounded-xl border border-gray-200 dark:border-zinc-800 p-4 sm:p-6 shadow-sm space-y-4">
            <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
              <div>
                <label for="barcodeIccid" class="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1">
                  {$t('barcode_iccid_label')}
                </label>
                <input
                  id="barcodeIccid"
                  type="text"
                  bind:this={iccidInput}
                  value={barcodeIccid}
                  oninput={onIccidInput}
                  onkeydown={(e) => { if (e.key === 'Enter') msisdnInput?.focus(); }}
                  inputmode="numeric"
                  maxlength="20"
                  placeholder={$t('barcode_iccid_ph')}
                  class="w-full rounded-lg border border-gray-300 dark:border-zinc-700 bg-white dark:bg-zinc-900 px-3 py-2 text-sm font-mono focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none"
                />
              </div>
              <div>
                <label for="barcodeMsisdn" class="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1">
                  {$t('barcode_msisdn_label')}
                </label>
                <input
                  id="barcodeMsisdn"
                  type="text"
                  bind:this={msisdnInput}
                  value={barcodeMsisdn}
                  oninput={onMsisdnInput}
                  onkeydown={(e) => { if (e.key === 'Enter') submitBarcodeScan(); }}
                  inputmode="numeric"
                  maxlength="11"
                  placeholder={$t('barcode_msisdn_ph')}
                  class="w-full rounded-lg border border-gray-300 dark:border-zinc-700 bg-white dark:bg-zinc-900 px-3 py-2 text-sm font-mono focus:ring-2 focus:ring-blue-500 focus:border-blue-500 outline-none"
                />
              </div>
            </div>

            <div class="flex items-center justify-between">
              <div class="text-sm text-gray-500 dark:text-gray-400">
                {#if barcodeLoading}
                  <span class="inline-flex items-center gap-1">
                    <Icon icon="carbon:loading" class="w-4 h-4 animate-spin" />
                    {$t('barcode_saving')}
                  </span>
                {:else}
                  {$t('barcode_scans_buffered', { n: barcodeScans.length })}
                {/if}
              </div>
              <div class="flex items-center gap-2">
                <button
                  onclick={clearBarcodeInputs}
                  disabled={(!barcodeIccid && !barcodeMsisdn) || barcodeImporting}
                  class="inline-flex items-center gap-2 px-4 py-2 rounded-lg text-sm font-medium
                         text-gray-700 dark:text-gray-200 bg-gray-100 dark:bg-zinc-800
                         hover:bg-gray-200 dark:hover:bg-zinc-700
                         disabled:opacity-50 disabled:cursor-not-allowed transition"
                >
                  <Icon icon="carbon:trash-can" class="w-4 h-4" />
                  {$t('barcode_clear')}
                </button>
                <button
                  onclick={importBarcodeScans}
                  disabled={barcodeScans.length === 0 || barcodeImporting}
                  class="inline-flex items-center gap-2 px-4 py-2 rounded-lg text-sm font-medium text-white bg-blue-600 hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed transition"
                >
                  {#if barcodeImporting}
                    <Icon icon="carbon:loading" class="w-4 h-4 animate-spin" />
                    {$t('barcode_importing')}
                  {:else}
                    <Icon icon="carbon:document-import" class="w-4 h-4" />
                    {$t('barcode_import_scans')}
                  {/if}
                </button>
              </div>
            </div>

            {#if barcodeScans.length > 0}
              <div class="overflow-x-auto rounded-lg border border-gray-200 dark:border-zinc-800">
                <table class="min-w-full text-sm">
                  <thead class="bg-gray-50 dark:bg-zinc-800 text-gray-600 dark:text-gray-400">
                    <tr>
                      <th class="px-3 py-2 text-left font-medium">#</th>
                      <th class="px-3 py-2 text-left font-medium">{$t('barcode_col_iccid')}</th>
                      <th class="px-3 py-2 text-left font-medium">{$t('barcode_col_msisdn')}</th>
                      <th class="px-3 py-2 text-left font-medium">{$t('barcode_col_current_phone')}</th>
                      <th class="px-3 py-2 text-left font-medium">{$t('barcode_col_scanned_at')}</th>
                    </tr>
                  </thead>
                  <tbody class="divide-y divide-gray-200 dark:divide-zinc-800">
                    {#each barcodeScans as scan, index}
                      <tr class="hover:bg-gray-50 dark:hover:bg-zinc-800/50">
                        <td class="px-3 py-2 font-mono text-xs text-gray-500 dark:text-gray-400">{index + 1}</td>
                        <td class="px-3 py-2 font-mono text-xs">{scan.iccid}</td>
                        <td class="px-3 py-2 font-mono text-xs">{scan.msisdn}</td>
                        <td class="px-3 py-2 font-mono text-xs">{scan.phone_number ?? '-'}</td>
                        <td class="px-3 py-2 text-gray-500 dark:text-gray-400">{scan.created_at ?? '-'}</td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
            {/if}
          </div>
        {/if}

        <!-- Shared results -->
        {#if taskStatus.results.length > 0}
          <div class="bg-white dark:bg-zinc-900 rounded-xl border border-gray-200 dark:border-zinc-800 p-4 sm:p-6 shadow-sm">
            <h3 class="text-sm font-semibold text-gray-800 dark:text-gray-200 mb-3">{$t('phone_col_status')}</h3>
            <div class="overflow-x-auto rounded-lg border border-gray-200 dark:border-zinc-800 max-h-96 overflow-y-auto">
              <table class="min-w-full text-sm">
                <thead class="bg-gray-50 dark:bg-zinc-800 text-gray-600 dark:text-gray-400 sticky top-0">
                  <tr>
                    <th class="px-3 py-2 text-left font-medium">{$t('phone_col_com')}</th>
                    <th class="px-3 py-2 text-left font-medium">{$t('phone_col_iccid')}</th>
                    <th class="px-3 py-2 text-left font-medium">{$t('phone_col_current_phone')}</th>
                    <th class="px-3 py-2 text-left font-medium">{$t('phone_col_status')}</th>
                    <th class="px-3 py-2 text-left font-medium">Message</th>
                  </tr>
                </thead>
                <tbody class="divide-y divide-gray-200 dark:divide-zinc-800">
                  {#each taskStatus.results as r}
                    <tr class="hover:bg-gray-50 dark:hover:bg-zinc-800/50">
                      <td class="px-3 py-2">{displayPort(r.com_port) || '-'}</td>
                      <td class="px-3 py-2 font-mono text-xs">{r.sim_id}</td>
                      <td class="px-3 py-2">{formatPhone(r.phone_number)}</td>
                      <td class="px-3 py-2 {statusClass(r.status)}">{r.status}</td>
                      <td class="px-3 py-2 text-gray-600 dark:text-gray-400">{r.message}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          </div>
        {/if}
      </div>
    {/if}
  </div>
</div>

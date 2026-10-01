<script>
  import { onMount, onDestroy } from 'svelte';
  import Icon from '@iconify/svelte';
  import { apiClient } from '../js/api.js';
  import { t, lang } from '../js/i18n.js';
  import { getMccCountry } from '../js/country.js';
  import { displayPort } from '../js/modem.js';

  let { onBack = () => {} } = $props();

  let rows = $state([]);
  let loading = $state(true);
  let refreshing = $state(false);
  let error = $state('');
  let itemOptions = $state([]);
  let itemSearchKeyword = $state('');
  let isItemDropdownOpen = $state(false);
  let itemBoxEl = $state(); // DOM ref for click-outside detection
  let itemOptionsRequestId = 0;
  let selectedItemId = $state('');
  let selectedItemName = $state('');
  let unitPriceInput = $state('');
  let selectedSuccessCount = $state(0);
  let selectedEarningLoading = $state(false);
  let platformPrices = $state([]);
  let selectedPlatformPriceId = $state(null);
  let platformPriceSearchKeyword = $state('');
  let isPlatformPriceDropdownOpen = $state(false);
  let platformPriceBoxEl = $state(); // DOM ref for click-outside detection
  let priceSaving = $state(false);
  let priceSaveError = $state('');
  let priceSaved = $state(false);
  let priceSavedTimer;

  let detailRow = $state(null);
  let detailMode = $state('item'); // 'item' | 'sim'
  let detailPhoneNumber = $state('');
  let detailSms = $state([]);
  let detailLoading = $state(false);

  let pollTimer;

  function formatMoney(value) {
    const num = Number(value || 0);
    if (!Number.isFinite(num)) return '0';
    return num.toFixed(2).replace(/\.00$/, '');
  }

  function parseUnitPrice(value) {
    const n = Number(value);
    if (!Number.isFinite(n) || n < 0) return null;
    return n;
  }

  const countryNameAliases = { uk: 'england' };
  function normalizeCountryName(name) {
    const key = String(name || '').toLowerCase().replace(/[^a-z]/g, '');
    return countryNameAliases[key] ?? key;
  }

  const selectedUnitPrice = $derived(parseUnitPrice(unitPriceInput));
  const selectedEarning = $derived(
    selectedUnitPrice == null ? 0 : selectedSuccessCount * selectedUnitPrice
  );

  // Countries actually present in the stats table (from each SIM's platform country_code)
  const usedCountryCodes = $derived(
    new Set(rows.map((r) => r.country_code).filter((c) => c))
  );
  // Fallback for SIMs not yet platform-connected: match by the IMSI-derived country name
  // shown in the 国家 column (e.g. UK/英国) against the platform price's country_title.
  const usedCountryNames = $derived(
    new Set(
      rows
        .filter((r) => !r.country_code)
        .map((r) => normalizeCountryName(getMccCountry(r.imsi, 'en')))
        .filter((name) => name)
    )
  );
  const filteredPlatformPrices = $derived(
    usedCountryCodes.size > 0
      ? platformPrices.filter((p) => usedCountryCodes.has(p.country_id))
      : usedCountryNames.size > 0
        ? platformPrices.filter((p) => {
            const title = normalizeCountryName(p.country_title || '');
            return [...usedCountryNames].some((name) => title.includes(name));
          })
        : platformPrices
  );
  const platformPriceOptions = $derived(
    (() => {
      const keyword = platformPriceSearchKeyword.trim().toLowerCase();
      if (!keyword) return platformPrices;
      return platformPrices.filter((p) =>
        `${p.country_id ?? ''} ${p.country_title ?? ''}`.toLowerCase().includes(keyword)
      );
    })()
  );
  const selectedPlatformPrice = $derived(
    platformPrices.find((p) => p.country_id === selectedPlatformPriceId) ?? filteredPlatformPrices[0]
  );

  async function fetchData() {
    try {
      error = '';
      const data = await apiClient.getFirefoxMoneyStats();
      rows = Array.isArray(data) ? data : (data?.data ?? []);
    } catch (e) {
      error = e?.message ?? $t('err_load_money_stats');
    } finally {
      loading = false;
    }
  }

  async function handleRefresh() {
    if (refreshing) return;
    refreshing = true;
    try {
      await fetchData();
    } finally {
      refreshing = false;
    }
  }

  async function fetchItemOptions(keyword = '') {
    const requestId = ++itemOptionsRequestId;
    try {
      const data = await apiClient.getFirefoxMoneyItems(keyword, 300);
      if (requestId !== itemOptionsRequestId) return; // a newer request superseded this one
      itemOptions = Array.isArray(data) ? data : (data?.data ?? []);
    } catch (e) {
      if (requestId !== itemOptionsRequestId) return;
      console.warn('Failed to load money item options:', e);
      itemOptions = [];
    }
  }

  async function fetchSelectedItemEarning() {
    selectedSuccessCount = 0;
    if (!selectedItemId) return;

    selectedEarningLoading = true;
    try {
      const data = await apiClient.getFirefoxMoneyItemEarning(selectedItemId);
      const payload = data?.data ?? data;
      selectedSuccessCount = Number(payload?.success_count ?? 0);
      if (!Number.isFinite(selectedSuccessCount)) {
        selectedSuccessCount = 0;
      }
    } catch (e) {
      console.warn('Failed to load selected item earning stats:', e);
      selectedSuccessCount = 0;
    } finally {
      selectedEarningLoading = false;
    }
  }

  async function fetchPlatformPrices(itemId) {
    platformPrices = [];
    selectedPlatformPriceId = null;
    if (!itemId) return;
    try {
      const data = await apiClient.getFirefoxMoneyItemPlatformPrices(itemId);
      platformPrices = Array.isArray(data) ? data : (data?.data ?? []);
    } catch (e) {
      console.warn('Failed to load platform reference prices:', e);
      platformPrices = [];
    }
  }

  function selectItem(opt) {
    selectedItemId = opt.item_id;
    selectedItemName = opt.item_name || '';
    unitPriceInput = String(opt.seller_item_price ?? 0);
    fetchSelectedItemEarning();
    fetchPlatformPrices(opt.item_id);
  }

  function onChooseItem(opt) {
    selectItem(opt);
    isItemDropdownOpen = false;
  }

  function toggleItemDropdown() {
    if (isItemDropdownOpen) {
      isItemDropdownOpen = false;
      return;
    }
    isItemDropdownOpen = true;
    itemSearchKeyword = '';
    fetchItemOptions('');
  }

  function onItemSearchInput(event) {
    itemSearchKeyword = event.currentTarget?.value || '';
    fetchItemOptions(itemSearchKeyword);
  }

  function onChoosePlatformPrice(opt) {
    selectedPlatformPriceId = opt.country_id;
    isPlatformPriceDropdownOpen = false;
  }

  function togglePlatformPriceDropdown() {
    if (isPlatformPriceDropdownOpen) {
      isPlatformPriceDropdownOpen = false;
      return;
    }
    isPlatformPriceDropdownOpen = true;
    platformPriceSearchKeyword = '';
  }

  function onPlatformPriceSearchInput(event) {
    platformPriceSearchKeyword = event.currentTarget?.value || '';
  }

  function onDocumentClick(event) {
    if (isItemDropdownOpen && itemBoxEl && !itemBoxEl.contains(event.target)) {
      isItemDropdownOpen = false;
    }
    if (isPlatformPriceDropdownOpen && platformPriceBoxEl && !platformPriceBoxEl.contains(event.target)) {
      isPlatformPriceDropdownOpen = false;
    }
  }

  function onUnitPriceInput(event) {
    const raw = event.currentTarget?.value || '';
    const cleaned = raw.replace(/[^\d.]/g, '');
    const firstDotIndex = cleaned.indexOf('.');
    unitPriceInput = firstDotIndex === -1
      ? cleaned
      : cleaned.slice(0, firstDotIndex + 1) + cleaned.slice(firstDotIndex + 1).replace(/\./g, '');
    if (raw !== unitPriceInput) {
      event.currentTarget.value = unitPriceInput;
    }
  }

  async function saveUnitPrice() {
    if (!selectedItemId) return;
    const price = parseUnitPrice(unitPriceInput);
    if (price == null) return;

    priceSaving = true;
    priceSaveError = '';
    try {
      await apiClient.updateFirefoxMoneyItemPrice(selectedItemId, price);
      await Promise.all([fetchData(), fetchSelectedItemEarning()]);
      priceSaved = true;
      clearTimeout(priceSavedTimer);
      priceSavedTimer = setTimeout(() => {
        priceSaved = false;
      }, 2000);
    } catch (e) {
      priceSaveError = e?.message ?? $t('err_save_item_price');
    } finally {
      priceSaving = false;
    }
  }

  function onUnitPriceKeydown(event) {
    if (event.key === 'Enter') {
      event.preventDefault();
      saveUnitPrice();
    }
  }

  function focusOnMount(node) {
    node.focus();
  }

  async function openSmsDetail() {
    if (!selectedItemId) return;
    detailMode = 'item';
    detailRow = selectedItemId;
    detailSms = [];

    detailLoading = true;
    try {
      const data = await apiClient.getFirefoxMoneySmsDetailByItem(selectedItemId);
      detailSms = Array.isArray(data) ? data : (data?.data ?? []);
    } catch (e) {
      console.warn('Failed to load SMS detail:', e);
      detailSms = [];
    } finally {
      detailLoading = false;
    }
  }

  async function openSmsDetailForRow(row) {
    if (!row?.sim_id) return;
    detailMode = 'sim';
    detailRow = row.sim_id;
    detailPhoneNumber = row.phone_number || '—';
    detailSms = [];

    detailLoading = true;
    try {
      const data = await apiClient.getFirefoxMoneySmsDetailBySim(row.sim_id);
      detailSms = Array.isArray(data) ? data : (data?.data ?? []);
    } catch (e) {
      console.warn('Failed to load SMS detail:', e);
      detailSms = [];
    } finally {
      detailLoading = false;
    }
  }

  function closeSmsDetail() {
    detailRow = null;
    detailSms = [];
  }

  function formatDateTime(ts) {
    if (!ts) return '-';
    return new Date(ts).toLocaleString();
  }

  const detailSuccessCount = $derived(detailSms.filter((s) => s.success).length);
  const detailFailedCount = $derived(detailSms.length - detailSuccessCount);

  onMount(async () => {
    const [, defaultOptions] = await Promise.all([fetchData(), apiClient.getFirefoxMoneyItems('', 300)]);
    const list = Array.isArray(defaultOptions) ? defaultOptions : (defaultOptions?.data ?? []);
    if (list.length > 0) {
      selectItem(list[0]);
    }
    pollTimer = setInterval(fetchData, 5000);
  });

  onDestroy(() => {
    if (pollTimer) clearInterval(pollTimer);
    clearTimeout(priceSavedTimer);
  });
</script>

<svelte:window onclick={onDocumentClick} />


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
      <h1 class="text-sm font-semibold tracking-wide">{$t('money_page_title')}</h1>
    </div>

    <button
      onclick={handleRefresh}
      disabled={refreshing}
      class="inline-flex items-center gap-1.5 rounded-md bg-blue-600 px-2.5 py-1.5 text-[11px] font-semibold text-white hover:bg-blue-700 transition disabled:opacity-60"
    >
      <Icon icon="carbon:renew" class="h-3.5 w-3.5 {refreshing ? 'animate-spin' : ''}" />
      {$t('btn_refresh')}
    </button>
  </header>

  <div class="flex-1 overflow-auto p-0">
    {#if loading}
      <div class="flex h-48 items-center justify-center text-gray-500 dark:text-gray-400">
        <Icon icon="carbon:loading" class="mr-2 h-6 w-6 animate-spin" />
        {$t('loading')}
      </div>
    {:else if error}
      <div class="rounded-lg border border-red-300 bg-red-50 px-4 py-3 text-sm text-red-700 dark:border-red-800 dark:bg-red-900/20 dark:text-red-300">
        {error}
      </div>
    {:else}
      <section class="border-b border-gray-300 bg-white px-3 py-3 dark:border-zinc-700 dark:bg-zinc-800">
        <div class="flex flex-wrap items-start gap-4">
        <div class="max-w-3xl rounded border border-gray-300 dark:border-zinc-600">
          <div class="flex flex-wrap items-stretch">
            <label for="money-item-input" class="flex items-center bg-gray-100 px-3 py-2 text-sm font-medium text-gray-700 dark:bg-zinc-700 dark:text-gray-200">
              {$t('money_item_label')}
            </label>
            <div class="relative min-w-[220px] flex-1 p-2" bind:this={itemBoxEl}>
              <button
                id="money-item-toggle"
                type="button"
                onclick={toggleItemDropdown}
                class="flex w-full items-center justify-between rounded border border-gray-300 px-2 py-1.5 text-left text-sm outline-none focus:border-blue-500 dark:border-zinc-600 dark:bg-zinc-900"
              >
                <span class="truncate">
                  {selectedItemId ? `${selectedItemId} | ${selectedItemName}` : $t('money_item_no_selection')}
                </span>
                <Icon
                  icon="carbon:chevron-down"
                  class="h-4 w-4 shrink-0 text-gray-500 transition-transform dark:text-gray-400 {isItemDropdownOpen ? 'rotate-180' : ''}"
                />
              </button>
              {#if isItemDropdownOpen}
                <div class="absolute left-2 right-2 top-full z-20 mt-1 rounded border border-gray-300 bg-white shadow-lg dark:border-zinc-600 dark:bg-zinc-800">
                  <div class="border-b border-gray-200 p-2 dark:border-zinc-700">
                    <input
                      id="money-item-search"
                      type="text"
                      autocomplete="off"
                      use:focusOnMount
                      class="w-full rounded border border-gray-300 px-2 py-1.5 text-sm outline-none focus:border-blue-500 dark:border-zinc-600 dark:bg-zinc-900"
                      placeholder={$t('money_item_search_placeholder')}
                      value={itemSearchKeyword}
                      oninput={onItemSearchInput}
                    />
                  </div>
                  <div class="max-h-56 overflow-y-auto">
                    {#each itemOptions as opt (opt.item_id)}
                      <button
                        type="button"
                        onclick={() => onChooseItem(opt)}
                        class="block w-full px-3 py-1.5 text-left text-sm transition
                               {selectedItemId === opt.item_id
                                 ? 'bg-blue-600 text-white'
                                 : 'text-gray-700 hover:bg-blue-50 dark:text-gray-200 dark:hover:bg-zinc-700'}"
                      >
                        {opt.item_id} | {opt.item_name}
                      </button>
                    {:else}
                      <div class="px-3 py-2 text-sm text-gray-500 dark:text-gray-400">
                        {$t('money_item_select_placeholder')}
                      </div>
                    {/each}
                  </div>
                </div>
              {/if}
            </div>
            <label for="money-unit-price" class="flex items-center border-l border-gray-300 bg-gray-100 px-3 py-2 text-sm font-medium text-gray-700 dark:border-zinc-600 dark:bg-zinc-700 dark:text-gray-200">
              {$t('money_unit_price_label')}
            </label>
            <div class="flex items-center gap-2 border-l border-gray-300 p-2 dark:border-zinc-600">
              <input
                id="money-unit-price"
                type="text"
                inputmode="decimal"
                class="w-24 rounded border border-gray-300 px-2 py-1.5 text-sm outline-none focus:border-blue-500 dark:border-zinc-600 dark:bg-zinc-900"
                placeholder={$t('money_unit_price_placeholder')}
                value={unitPriceInput}
                oninput={onUnitPriceInput}
                onkeydown={onUnitPriceKeydown}
              />
              <button
                type="button"
                onclick={saveUnitPrice}
                class="inline-flex items-center rounded bg-blue-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-blue-700 transition"
              >
                {$t('btn_confirm')}
              </button>
            </div>
          </div>
        </div>

        {#if selectedItemId}
          <div class="ml-auto rounded border border-gray-300 dark:border-zinc-600">
            <div class="flex items-stretch">
              <span class="flex items-center bg-gray-100 px-3 py-2 text-sm font-medium text-gray-700 dark:bg-zinc-700 dark:text-gray-200">
                {$t('money_platform_price_label')}
              </span>
              <div class="flex items-center gap-2 border-l border-gray-300 p-2 text-xs dark:border-zinc-600">
                {#if filteredPlatformPrices.length === 0}
                  <span class="text-gray-400 dark:text-gray-500">{$t('money_platform_price_none')}</span>
                {:else}
                  <div class="relative min-w-[180px]" bind:this={platformPriceBoxEl}>
                    <button
                      type="button"
                      onclick={togglePlatformPriceDropdown}
                      class="flex w-full items-center justify-between rounded border border-gray-300 px-2 py-1.5 text-xs outline-none focus:border-blue-500 dark:border-zinc-600 dark:bg-zinc-900"
                    >
                      <span class="truncate">
                        {selectedPlatformPrice?.country_title || selectedPlatformPrice?.country_id || '-'}
                      </span>
                      <Icon
                        icon="carbon:chevron-down"
                        class="h-4 w-4 shrink-0 text-gray-500 transition-transform dark:text-gray-400 {isPlatformPriceDropdownOpen ? 'rotate-180' : ''}"
                      />
                    </button>
                    {#if isPlatformPriceDropdownOpen}
                      <div class="absolute left-0 right-0 top-full z-20 mt-1 rounded border border-gray-300 bg-white shadow-lg dark:border-zinc-600 dark:bg-zinc-800">
                        <div class="border-b border-gray-200 p-2 dark:border-zinc-700">
                          <input
                            type="text"
                            autocomplete="off"
                            use:focusOnMount
                            class="w-full rounded border border-gray-300 px-2 py-1.5 text-xs outline-none focus:border-blue-500 dark:border-zinc-600 dark:bg-zinc-900"
                            placeholder={$t('money_platform_price_search_placeholder')}
                            value={platformPriceSearchKeyword}
                            oninput={onPlatformPriceSearchInput}
                          />
                        </div>
                        <div class="max-h-56 overflow-y-auto">
                          {#each platformPriceOptions as p (p.country_id ?? p.country_title)}
                            <button
                              type="button"
                              onclick={() => onChoosePlatformPrice(p)}
                              class="block w-full px-3 py-1.5 text-left text-xs transition
                                     {selectedPlatformPrice?.country_id === p.country_id
                                       ? 'bg-blue-600 text-white'
                                       : 'text-gray-700 hover:bg-blue-50 dark:text-gray-200 dark:hover:bg-zinc-700'}"
                            >
                              {p.country_title || p.country_id || '-'}
                            </button>
                          {:else}
                            <div class="px-3 py-2 text-xs text-gray-500 dark:text-gray-400">
                              {$t('money_platform_price_no_match')}
                            </div>
                          {/each}
                        </div>
                      </div>
                    {/if}
                  </div>
                  <span class="font-semibold text-gray-700 dark:text-gray-200">
                    {formatMoney(selectedPlatformPrice?.item_uprice)}
                  </span>
                {/if}
              </div>
            </div>
          </div>
        {/if}
        </div>

        {#if priceSaving}
          <div class="mt-1 text-xs text-gray-500 dark:text-gray-400">{$t('saving')}</div>
        {:else if priceSaveError}
          <div class="mt-1 text-xs text-red-600 dark:text-red-400">{priceSaveError}</div>
        {:else if priceSaved}
          <div class="mt-1 text-xs text-[#10a248] dark:text-green-300">{$t('money_price_saved')}</div>
        {/if}

        {#if selectedItemId}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
          <div
            class="mt-3 flex cursor-pointer flex-wrap items-center gap-x-4 gap-y-1 rounded border border-blue-200 bg-blue-50 px-3 py-2 text-sm text-blue-800 hover:bg-blue-100 dark:border-blue-700 dark:bg-blue-900/25 dark:text-blue-200 dark:hover:bg-blue-900/40"
            role="button"
            tabindex="0"
            ondblclick={openSmsDetail}
          >
            <div class="font-semibold">
              {$t('money_selected_item_result_title')}
            </div>
            <div class="text-xs">
              {$t('money_selected_item_result_item')}: {selectedItemId}{selectedItemName ? ` | ${selectedItemName}` : ''}
            </div>
            <div class="text-xs">
              {$t('money_selected_item_result_upload_count')}: {selectedEarningLoading ? '...' : selectedSuccessCount}
            </div>
            <div class="text-sm font-bold text-[#10a248] dark:text-green-300">
              {$t('money_selected_item_result_earning')}: {formatMoney(selectedEarning)}
            </div>
          </div>
        {/if}
      </section>

      <section class="overflow-hidden border-y border-gray-300 bg-white dark:border-zinc-700 dark:bg-zinc-800">
        <div class="overflow-x-auto">
          <table class="money-table min-w-full text-[13px] leading-none">
            <colgroup>
              <col class="col-port" />
              <col class="col-phone" />
              <col class="col-country" />
              <col class="col-platform" />
              <col class="col-wait" />
              <col class="col-recv" />
              <col class="col-up" />
              <col class="col-fail" />
              <col class="col-profit" />
              <col class="col-items" />
            </colgroup>
            <thead class="bg-[#17add4] text-white">
              <tr>
                <th class="px-3 py-2.5 text-left font-semibold">{$t('money_col_port')}</th>
                <th class="px-3 py-2.5 text-left font-semibold">{$t('money_col_phone')}</th>
                <th class="px-3 py-2.5 text-left font-semibold">{$t('money_col_country')}</th>
                <th class="px-3 py-2.5 text-left font-semibold">{$t('money_col_platform_status')}</th>
                <th class="px-3 py-2.5 text-center font-semibold">{$t('money_col_waiting')}</th>
                <th class="px-3 py-2.5 text-center font-semibold">{$t('money_col_received')}</th>
                <th class="px-3 py-2.5 text-center font-semibold">{$t('money_col_uploaded')}</th>
                <th class="px-3 py-2.5 text-center font-semibold">{$t('money_col_failed')}</th>
                <th class="px-3 py-2.5 text-center font-semibold">{$t('money_col_earning')}</th>
                <th class="px-3 py-2.5 text-left font-semibold">{$t('money_col_earning_items')}</th>
              </tr>
            </thead>
            <tbody>
              {#each rows as row}
                <!-- svelte-ignore a11y_click_events_have_key_events -->
                <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
                <tr
                  class="cursor-pointer bg-white border-b border-gray-200 hover:bg-blue-50 dark:bg-zinc-800 dark:border-zinc-700 dark:hover:bg-zinc-700/50"
                  role="button"
                  tabindex="0"
                  ondblclick={() => openSmsDetailForRow(row)}
                >
                  <td class="whitespace-nowrap px-3 py-2.5 font-medium">{displayPort(row.com_port) || '—'}</td>
                  <td class="whitespace-nowrap px-3 py-2.5">{row.phone_number || '—'}</td>
                  <td class="whitespace-nowrap px-3 py-2.5">{row.imsi ? getMccCountry(row.imsi, $lang) : '—'}</td>
                  <td class="whitespace-nowrap px-3 py-2.5">
                    {#if row.platform_connected}
                      <span class="inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-xs font-medium bg-green-100 dark:bg-green-900/30 text-green-700 dark:text-green-400">
                        {$t('platform_connected')}
                      </span>
                    {:else}
                      <span class="text-gray-400 dark:text-gray-500 text-xs">{$t('platform_not_connected')}</span>
                    {/if}
                  </td>
                  <td class="px-3 py-2.5 text-center font-semibold text-[#245dff] dark:text-blue-300">{row.waiting_sms_count ?? 0}</td>
                  <td class="px-3 py-2.5 text-center font-medium">{row.received_sms_count ?? 0}</td>
                  <td class="px-3 py-2.5 text-center font-semibold text-[#245dff] dark:text-blue-300">{row.successful_uploaded_sms_count ?? 0}</td>
                  <td class="px-3 py-2.5 text-center font-semibold text-[#d63031] dark:text-red-300">{row.failed_sms_count ?? 0}</td>
                  <td class="px-3 py-2.5 text-center font-bold text-[#10a248] dark:text-green-300">{formatMoney(row.money_earning ?? 0)}</td>
                  <td class="px-3 py-2.5 text-[12px] leading-5">{row.earning_item_names || '—'}</td>
                </tr>
              {:else}
                <tr>
                  <td colspan="10" class="px-3 py-8 text-center text-gray-500 dark:text-gray-400">{$t('money_empty')}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      </section>
    {/if}
  </div>

  {#if detailRow}
    <div
      class="fixed inset-0 z-30 flex items-center justify-center bg-black/40 p-4"
      onclick={closeSmsDetail}
      onkeydown={(e) => e.key === 'Escape' && closeSmsDetail()}
      role="button"
      tabindex="-1"
    >
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
      <div
        class="flex max-h-[80vh] w-full max-w-5xl flex-col rounded-lg bg-white shadow-xl dark:bg-zinc-800"
        onclick={(e) => e.stopPropagation()}
        role="document"
      >
        <div class="flex items-start justify-between border-b border-gray-200 px-4 py-3 dark:border-zinc-700">
          <div>
            <div class="font-semibold">{$t('money_sms_detail_title')}</div>
            <div class="mt-1 text-xs text-gray-500 dark:text-gray-400">
              {#if detailMode === 'sim'}
                {$t('money_sms_detail_subtitle_phone', {
                  phone: detailPhoneNumber || '—',
                  attempts: detailSms.length,
                  success: detailSuccessCount,
                  failed: detailFailedCount,
                })}
              {:else}
                {$t('money_sms_detail_subtitle', {
                  item: selectedItemId ? `${selectedItemId}${selectedItemName ? ` | ${selectedItemName}` : ''}` : '—',
                  attempts: detailSms.length,
                  success: detailSuccessCount,
                  failed: detailFailedCount,
                })}
              {/if}
            </div>
          </div>
          <button
            onclick={closeSmsDetail}
            class="rounded p-1 text-gray-400 hover:bg-gray-100 hover:text-gray-600 dark:hover:bg-zinc-700 dark:hover:text-gray-200"
            aria-label={$t('close')}
          >
            <Icon icon="carbon:close" class="h-5 w-5" />
          </button>
        </div>
        <div class="overflow-y-auto">
          {#if detailLoading}
            <div class="flex h-40 items-center justify-center text-gray-500 dark:text-gray-400">
              <Icon icon="carbon:loading" class="mr-2 h-6 w-6 animate-spin" />
              {$t('loading')}
            </div>
          {:else}
            <table class="min-w-full text-sm">
              <thead class="bg-gray-50 dark:bg-zinc-700/50">
                <tr>
                  <th class="px-4 py-2 text-left font-medium">{$t('col_time')}</th>
                  <th class="px-4 py-2 text-left font-medium">{$t('col_phone_number')}</th>
                  <th class="px-4 py-2 text-left font-medium">{$t('col_status')}</th>
                  <th class="px-4 py-2 text-left font-medium">{$t('col_message')}</th>
                  <th class="px-4 py-2 text-left font-medium">{$t('col_response')}</th>
                  <th class="px-4 py-2 text-right font-medium">{$t('money_sms_detail_col_money')}</th>
                </tr>
              </thead>
              <tbody class="divide-y divide-gray-100 dark:divide-zinc-700">
                {#each detailSms as sms (sms.id)}
                  <tr class="align-top">
                    <td class="whitespace-nowrap px-4 py-3 text-xs text-gray-500 dark:text-gray-400">
                      {formatDateTime(sms.timestamp)}
                    </td>
                    <td class="whitespace-nowrap px-4 py-3">{sms.phone_number || '—'}</td>
                    <td class="px-4 py-3">
                      <span class={`inline-flex rounded-full px-2 py-0.5 text-xs font-medium ${sms.success
                        ? 'bg-green-100 text-green-700 dark:bg-green-900/30 dark:text-green-300'
                        : 'bg-red-100 text-red-700 dark:bg-red-900/30 dark:text-red-300'}`}>
                        {sms.success ? $t('status_success') : $t('status_failed')}
                      </span>
                    </td>
                    <td class="max-w-[16rem] break-words px-4 py-3">{sms.message}</td>
                    <td class="max-w-[16rem] whitespace-pre-wrap break-words px-4 py-3 text-xs text-gray-500 dark:text-gray-400">
                      {sms.platform_response || '-'}
                    </td>
                    <td class="px-4 py-3 text-right font-semibold text-[#10a248] dark:text-green-300">
                      {formatMoney(sms.money ?? 0)}
                    </td>
                  </tr>
                {:else}
                  <tr>
                    <td colspan="6" class="px-4 py-8 text-center text-gray-500 dark:text-gray-400">
                      {$t('money_sms_detail_empty')}
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          {/if}
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .money-table thead tr {
    height: 36px;
  }

  .money-table tbody tr {
    height: 34px;
  }

  .money-table th,
  .money-table td {
    font-variant-numeric: tabular-nums;
  }

  .money-table .col-port {
    width: 90px;
  }

  .money-table .col-phone {
    width: 160px;
  }

  .money-table .col-country {
    width: 100px;
  }

  .money-table .col-platform {
    width: 110px;
  }

  .money-table .col-wait,
  .money-table .col-recv,
  .money-table .col-up,
  .money-table .col-fail {
    width: 92px;
  }

  .money-table .col-profit {
    width: 110px;
  }

  .money-table .col-items {
    min-width: 420px;
  }
</style>

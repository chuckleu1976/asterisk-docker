<script>
  import { onMount } from 'svelte';
  import Icon from '@iconify/svelte';
  import { apiClient } from '../js/api.js';
  import { t, lang } from '../js/i18n.js';
  import { getMccCountry } from '../js/country.js';
  import { displayPort } from '../js/modem.js';

  let { onBack = () => {} } = $props();

  // ── State ──────────────────────────────────────────────────────────────────
  let apiKey = $state('');
  let savingKey = $state(false);
  let keyMessage = $state('');
  let keyError = $state('');

  let countries = $state([]);
  let selectedCountry = $state('');
  let countryFilter = $state('');

  const sortedCountries = $derived(
    [...countries].sort((a, b) => (a.name ?? '').localeCompare(b.name ?? '')),
  );

  const filteredCountries = $derived(
    countryFilter
      ? sortedCountries.filter(c =>
          c.prefix?.toLowerCase().includes(countryFilter.toLowerCase()) ||
          c.name?.toLowerCase().includes(countryFilter.toLowerCase()) ||
          c.id?.toLowerCase().includes(countryFilter.toLowerCase())
        )
      : sortedCountries,
  );

  $effect(() => {
    if (countryFilter && selectedCountry && filteredCountries.length > 0) {
      if (!filteredCountries.some(c => c.id === selectedCountry)) {
        selectedCountry = filteredCountries[0].id;
      }
    }
  });

  let sims = $state([]);
  let simCards = $state([]);
  let selected = $state(new Set());
  let loading = $state(true);
  let error = $state('');

  let uploading = $state(false);
  let uploadResult = $state(null);
  let uploadError = $state('');

  let deleting = $state(false);
  let deleteResult = $state('');
  let deleteError = $state('');

  // Prefer a country default inferred from SIM MCC (e.g. UK MCC 234/235 -> +44).
  const mccToDialPrefix = {
    '234': '+44',
    '235': '+44',
    '460': '+86',
    '461': '+86',
    '454': '+852',
    '455': '+853',
    '262': '+49',
    '208': '+33',
    '222': '+39',
    '214': '+34',
    '310': '+1',
    '311': '+1',
    '312': '+1',
    '313': '+1',
    '314': '+1',
    '315': '+1',
    '316': '+1',
  };

  function normalizePrefix(prefix) {
    return String(prefix ?? '').replace(/\s+/g, '');
  }

  function matchCountryByDialPrefix(targetPrefix, list) {
    if (!targetPrefix) return null;
    const normalizedTarget = normalizePrefix(targetPrefix);
    return list.find(c => {
      const p = normalizePrefix(c.prefix);
      return p === normalizedTarget || p.startsWith(`${normalizedTarget}/`) || p.startsWith(`${normalizedTarget},`);
    }) ?? null;
  }

  function findCountryBySimMcc(simList, countryList) {
    for (const sim of simList) {
      const imsi = String(sim?.imsi || sim?.card?.imsi || sim?.operator_info?.operator_id || '').trim();
      if (imsi.length < 3) continue;
      const mcc = imsi.slice(0, 3);
      const dialPrefix = mccToDialPrefix[mcc];
      if (!dialPrefix) continue;

      const matched = matchCountryByDialPrefix(dialPrefix, countryList);
      if (matched?.id) return matched.id;
    }
    return '';
  }

  const allowedNetworkStatuses = new Set(['1', '5']);

  function getNetworkStatusCode(sim) {
    return String(sim?.network_registration?.status ?? '').trim();
  }

  function simPhone(sim) {
    return sim?.phone_number || sim?.card?.phone_number || '';
  }

  function simImsi(sim) {
    return sim?.imsi || sim?.card?.imsi || sim?.operator_info?.operator_id || '';
  }

  function canUploadToPlatform(sim) {
    return !!simPhone(sim) && allowedNetworkStatuses.has(getNetworkStatusCode(sim));
  }

  function getNetworkStatusLabel(sim) {
    switch (getNetworkStatusCode(sim)) {
      case '1':
        return $t('net_reg_home');
      case '2':
        return $t('net_searching');
      case '3':
        return $t('net_reg_denied');
      case '5':
        return $t('net_reg_roaming');
      case '0':
        return $t('net_not_registered');
      case '4':
        return $t('net_unknown');
      default:
        return $t('unknown');
    }
  }

  // ── Data fetching ──────────────────────────────────────────────────────────
  async function fetchData() {
    loading = true;
    error = '';
    try {
      const [keyRes, countriesRes, infoRes, cardsRes] = await Promise.all([
        apiClient.getFirefoxApiKey(),
        apiClient.getFirefoxCountries(),
        apiClient.getAllSimsInfo(),
        apiClient.getAllSimCards(),
      ]);
      apiKey = keyRes?.data?.api_key ?? '';
      countries = Array.isArray(countriesRes?.data) ? countriesRes.data : [];
      const infos = Array.isArray(infoRes) ? infoRes : (infoRes?.data ?? []);
      const cards = Array.isArray(cardsRes) ? cardsRes : (cardsRes?.data ?? []);
      simCards = cards;

      // Merge info and cards, keep only real SIMs
      const cardMap = Object.fromEntries(cards.map(c => [c.id, c]));
      sims = infos
        .filter(info => info.has_sim !== false && info.sim_id)
        .map(info => ({
          ...info,
          card: cardMap[info.sim_id] ?? {},
        }))
        .sort((a, b) => (a.com_port ?? '').localeCompare(b.com_port ?? '', undefined, { numeric: true }));

      // Default country selection if any SIM already has one
      if (!selectedCountry) {
        const firstWithCountry = sims.find(s => s.card?.country_code);
        if (firstWithCountry) {
          selectedCountry = firstWithCountry.card.country_code;
        } else {
          const mccDefaultCountry = findCountryBySimMcc(sims, countries);
          if (mccDefaultCountry) {
            selectedCountry = mccDefaultCountry;
          } else if (countries.length > 0) {
            selectedCountry = countries[0].id;
          }
        }
      }
    } catch (e) {
      error = e?.data?.error ?? e?.message ?? $t('err_load_platform');
    } finally {
      loading = false;
    }
  }

  onMount(fetchData);

  // ── Selection ──────────────────────────────────────────────────────────────
  function toggleSim(simId) {
    const sim = sims.find(item => item.sim_id === simId);
    if (!sim || !canUploadToPlatform(sim)) return;
    const next = new Set(selected);
    next.has(simId) ? next.delete(simId) : next.add(simId);
    selected = next;
  }

  function toggleAll() {
    if (selected.size === selectableCount) {
      selected = new Set();
    } else {
      selected = new Set(sims.filter(canUploadToPlatform).map(s => s.sim_id));
    }
  }

  // ── Helpers ────────────────────────────────────────────────────────────────
  const selectableCount = $derived(sims.filter(canUploadToPlatform).length);

  function countryName(code) {
    return countries.find(c => c.id === code)?.name ?? code;
  }

  async function saveApiKey() {
    keyMessage = '';
    keyError = '';
    savingKey = true;
    try {
      await apiClient.setFirefoxApiKey(apiKey.trim());
      keyMessage = $t('msg_key_saved');
    } catch (e) {
      keyError = e?.data?.error ?? e?.message ?? $t('err_key_save_failed');
    } finally {
      savingKey = false;
    }
  }

  async function handleDelete() {
    deleteResult = '';
    deleteError = '';
    if (!confirm($t('delete_platform_confirm'))) return;

    deleting = true;
    try {
      await apiClient.deleteAllFromPlatform();
      deleteResult = $t('msg_delete_success');
      selected = new Set();
      await fetchData();
    } catch (e) {
      deleteError = e?.data?.error ?? e?.message ?? $t('err_delete_failed');
    } finally {
      deleting = false;
    }
  }

  async function handleUpload() {
    uploadResult = null;
    uploadError = '';

    const selectedSims = sims.filter(sim => selected.has(sim.sim_id));
    const eligibleSimIds = selectedSims.filter(canUploadToPlatform).map(sim => sim.sim_id);

    if (selected.size === 0) {
      uploadError = $t('err_no_sims_selected');
      return;
    }
    if (eligibleSimIds.length !== selectedSims.length || eligibleSimIds.length === 0) {
      uploadError = $t('err_platform_requires_registered_network');
      return;
    }
    if (!selectedCountry) {
      uploadError = $t('err_no_country_selected');
      return;
    }
    if (!apiKey.trim()) {
      uploadError = $t('err_no_api_key');
      return;
    }

    uploading = true;
    try {
      const response = await apiClient.uploadToFirefox(
        eligibleSimIds,
        selectedCountry,
      );
      uploadResult = response.data ?? response;
      await fetchData(); // refresh country_code persistence
    } catch (e) {
      uploadError = e?.data?.error ?? e?.message ?? $t('err_upload_failed');
    } finally {
      uploading = false;
    }
  }
</script>

<div class="flex flex-col h-dvh w-screen bg-gray-50 dark:bg-zinc-950 font-sans">
  <!-- Header -->
  <header class="flex items-center gap-3 px-4 py-3 border-b border-gray-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 shadow-sm shrink-0">
    <button
      onclick={onBack}
      class="inline-flex items-center gap-1.5 h-9 px-2.5 rounded-full bg-blue-600 text-white
             shadow-sm shadow-blue-600/30 hover:bg-blue-700 transition"
      aria-label="Back"
    >
      <Icon icon="carbon:arrow-left" class="w-4 h-4" />
      <span class="text-xs font-semibold">{$t('btn_back')}</span>
    </button>
    <div class="flex items-center gap-2">
      <Icon icon="carbon:cloud-upload" class="w-5 h-5 text-gray-500 dark:text-gray-400" />
      <h1 class="text-base font-semibold text-gray-800 dark:text-gray-100">{$t('platform_connect_title')}</h1>
    </div>
  </header>

  <!-- Content -->
  <div class="flex-1 overflow-auto p-4 sm:p-8">
    {#if loading}
      <div class="max-w-3xl mx-auto space-y-4">
        <div class="bg-white dark:bg-zinc-900 rounded-xl border border-gray-200 dark:border-zinc-800 p-6 animate-pulse h-32"></div>
        <div class="bg-white dark:bg-zinc-900 rounded-xl border border-gray-200 dark:border-zinc-800 p-6 animate-pulse h-64"></div>
      </div>
    {:else if error}
      <div class="max-w-md mx-auto text-center">
        <Icon icon="carbon:warning" class="w-10 h-10 text-red-400 mx-auto mb-2" />
        <p class="text-red-500 font-medium">{error}</p>
        <button onclick={fetchData} class="mt-4 px-4 py-2 rounded-lg border border-gray-200 dark:border-zinc-700 text-sm">{$t('try_again')}</button>
      </div>
    {:else}
      <div class="max-w-4xl mx-auto space-y-6">
        <!-- API Key card -->
        <div class="bg-white dark:bg-zinc-900 rounded-xl border border-gray-200 dark:border-zinc-800 shadow-sm p-6">
          <h2 class="text-sm font-semibold text-gray-800 dark:text-gray-100 mb-1">{$t('platform_api_key_title')}</h2>
          <p class="text-xs text-gray-500 dark:text-gray-400 mb-4">{$t('platform_api_key_desc')}</p>

          <div class="flex gap-3">
            <input
              type="text"
              bind:value={apiKey}
              placeholder={$t('platform_api_key_ph')}
              class="flex-1 px-3 py-2 rounded-lg border border-gray-300 dark:border-zinc-700
                     bg-white dark:bg-zinc-950 text-gray-900 dark:text-gray-100 text-sm
                     focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
            <button
              onclick={saveApiKey}
              disabled={savingKey}
              class="px-4 py-2 rounded-lg bg-blue-600 hover:bg-blue-700 text-white text-sm font-medium
                     disabled:opacity-60 disabled:cursor-not-allowed inline-flex items-center gap-2"
            >
              {#if savingKey}
                <Icon icon="carbon:loading" class="w-4 h-4 animate-spin" />
              {/if}
              {$t('btn_save_key')}
            </button>
          </div>

          {#if keyMessage}
            <p class="mt-3 text-xs text-green-600 dark:text-green-400 flex items-center gap-1">
              <Icon icon="carbon:checkmark" class="w-4 h-4" /> {keyMessage}
            </p>
          {/if}
          {#if keyError}
            <p class="mt-3 text-xs text-red-600 dark:text-red-400 flex items-center gap-1">
              <Icon icon="carbon:warning" class="w-4 h-4" /> {keyError}
            </p>
          {/if}
        </div>

        <!-- Country + Upload card -->
        <div class="bg-white dark:bg-zinc-900 rounded-xl border border-gray-200 dark:border-zinc-800 shadow-sm p-6">
          <div class="flex flex-col sm:flex-row sm:items-end gap-4 mb-4">
            <div class="flex-1">
              <label for="country" class="block text-xs font-medium text-gray-700 dark:text-gray-300 mb-1.5">
                {$t('country_code_label')}
              </label>
              <div class="relative">
                <input
                  type="text"
                  bind:value={countryFilter}
                  placeholder={$t('country_search_ph')}
                  class="w-full px-3 py-2 pr-8 rounded-lg border border-gray-300 dark:border-zinc-700
                         bg-white dark:bg-zinc-950 text-gray-900 dark:text-gray-100 text-sm
                         focus:outline-none focus:ring-2 focus:ring-blue-500"
                />
                {#if countryFilter}
                  <button
                    onclick={() => countryFilter = ''}
                    class="absolute right-2 top-1/2 -translate-y-1/2 text-gray-400 hover:text-gray-600 dark:hover:text-gray-200"
                  >
                    <Icon icon="carbon:close" class="w-4 h-4" />
                  </button>
                {/if}
              </div>
              <select
                id="country"
                bind:value={selectedCountry}
                class="w-full px-3 py-2 rounded-lg border border-gray-300 dark:border-zinc-700
                       bg-white dark:bg-zinc-950 text-gray-900 dark:text-gray-100 text-sm
                       focus:outline-none focus:ring-2 focus:ring-blue-500 mt-1"
              >
                {#each filteredCountries as country}
                  <option value={country.id}>{country.prefix} {country.name} ({country.id})</option>
                {/each}
              </select>
            </div>
              <div class="flex gap-2">
                <button
                  onclick={handleUpload}
                  disabled={uploading || selected.size === 0}
                  class="px-5 py-2 rounded-lg bg-green-600 hover:bg-green-700 text-white text-sm font-medium
                         disabled:opacity-60 disabled:cursor-not-allowed inline-flex items-center justify-center gap-2"
                >
                  {#if uploading}
                    <Icon icon="carbon:loading" class="w-4 h-4 animate-spin" />
                    {$t('uploading')}
                  {:else}
                    <Icon icon="carbon:cloud-upload" class="w-4 h-4" />
                    {$t('btn_upload_platform')}
                  {/if}
                </button>
                <button
                  onclick={handleDelete}
                  disabled={deleting}
                  class="px-5 py-2 rounded-lg bg-red-600 hover:bg-red-700 text-white text-sm font-medium
                         disabled:opacity-60 disabled:cursor-not-allowed inline-flex items-center justify-center gap-2"
                >
                  {#if deleting}
                    <Icon icon="carbon:loading" class="w-4 h-4 animate-spin" />
                    {$t('deleting')}
                  {:else}
                    <Icon icon="carbon:delete" class="w-4 h-4" />
                    {$t('btn_delete_from_platform')}
                  {/if}
                </button>
              </div>
          </div>

          {#if uploadResult}
            <div class="mb-4 text-xs text-green-600 dark:text-green-400 bg-green-50 dark:bg-green-900/20 rounded-lg p-3">
              <p class="font-medium">{$t('msg_upload_success', { n: uploadResult.uploaded_count ?? 0 })}</p>
              {#if uploadResult.batch_ids?.length}
                <p class="mt-1">{$t('batch_ids')}: {uploadResult.batch_ids.join(', ')}</p>
              {/if}
            </div>
          {/if}
          {#if uploadError}
            <div class="mb-4 text-xs text-red-600 dark:text-red-400 bg-red-50 dark:bg-red-900/20 rounded-lg p-3 flex items-start gap-2">
              <Icon icon="carbon:warning" class="w-4 h-4 shrink-0 mt-0.5" />
              <span>{uploadError}</span>
            </div>
          {/if}

          {#if deleteResult}
            <div class="mb-4 text-xs text-green-600 dark:text-green-400 bg-green-50 dark:bg-green-900/20 rounded-lg p-3">
              {deleteResult}
            </div>
          {/if}
          {#if deleteError}
            <div class="mb-4 text-xs text-red-600 dark:text-red-400 bg-red-50 dark:bg-red-900/20 rounded-lg p-3 flex items-start gap-2">
              <Icon icon="carbon:warning" class="w-4 h-4 shrink-0 mt-0.5" />
              <span>{deleteError}</span>
            </div>
          {/if}

          <!-- SIM table -->
          <div class="border border-gray-200 dark:border-zinc-700 rounded-lg overflow-hidden">
            <table class="w-full text-xs">
              <thead class="bg-gray-100 dark:bg-zinc-800">
                <tr>
                  <th class="w-10 px-2 py-2 text-center">
                    <input
                      type="checkbox"
                      checked={selectableCount > 0 && selected.size === selectableCount}
                      onchange={toggleAll}
                      class="rounded accent-blue-500"
                    />
                  </th>
                  <th class="px-3 py-2 text-left font-semibold text-gray-600 dark:text-gray-300">{$t('col_com_port')}</th>
                  <th class="px-3 py-2 text-left font-semibold text-gray-600 dark:text-gray-300">{$t('col_phone_number')}</th>
                  <th class="px-3 py-2 text-left font-semibold text-gray-600 dark:text-gray-300">{$t('col_iccid')}</th>
                  <th class="px-3 py-2 text-left font-semibold text-gray-600 dark:text-gray-300">{$t('col_network_status')}</th>
                  <th class="px-3 py-2 text-left font-semibold text-gray-600 dark:text-gray-300">{$t('saved_country_label')}</th>
                </tr>
              </thead>
              <tbody>
                {#each sims as sim}
                  {@const hasPhone = !!simPhone(sim)}
                  {@const canUpload = canUploadToPlatform(sim)}
                  <tr class="border-t border-gray-100 dark:border-zinc-800 {(hasPhone && canUpload) ? '' : 'opacity-50'}">
                    <td class="px-2 py-2 text-center">
                      <input
                        type="checkbox"
                        checked={selected.has(sim.sim_id)}
                        disabled={!canUpload}
                        onchange={() => toggleSim(sim.sim_id)}
                        class="rounded accent-blue-500"
                      />
                    </td>
                    <td class="px-3 py-2 font-mono text-gray-700 dark:text-gray-200">{displayPort(sim.com_port) || '—'}</td>
                    <td class="px-3 py-2 font-mono text-gray-700 dark:text-gray-200">{simPhone(sim) || '—'}</td>
                    <td class="px-3 py-2 font-mono text-gray-500 dark:text-gray-400">{sim.sim_id}</td>
                    <td class="px-3 py-2 text-gray-600 dark:text-gray-300 whitespace-nowrap">{getNetworkStatusLabel(sim)}</td>
                    <td class="px-3 py-2 text-gray-600 dark:text-gray-300">{countryName(sim.card?.country_code) || (simImsi(sim) ? getMccCountry(simImsi(sim), $lang) : '—')}</td>
                  </tr>
                {:else}
                  <tr>
                    <td colspan="6" class="px-6 py-8 text-center text-gray-400">
                      <Icon icon="carbon:sim-card" class="w-8 h-8 mx-auto mb-2 opacity-40" />
                      <p>{$t('no_sim_cards')}</p>
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
          <p class="mt-2 text-xs text-gray-500 dark:text-gray-400">{$t('platform_sim_hint')}</p>
        </div>
      </div>
    {/if}
  </div>
</div>

<script>
  import { onMount } from 'svelte';
  import Icon from '@iconify/svelte';
  import { simCards, simCardActions } from '../stores/simcards.js';
  import { apiClient } from '../js/api.js';
  import SimCardTabs from '../components/sim-card/SimCardTabs.svelte';
  import SimCardTabContent from '../components/sim-card/SimCardTabContent.svelte';
  import { t } from '../js/i18n.js';

  let { onBack = () => {}, filterSimId = null } = $props();

  let activeSimId = $state(null);
  let tabDataCache = $state({});
  let isCurrentTabLoading = $state(false);

  onMount(async () => {
    await simCardActions.loadAll();
  });

  $effect(() => {
    if ($simCards.length > 0 && !activeSimId) {
      const target = filterSimId && $simCards.find(s => s.id === filterSimId)
        ? filterSimId
        : $simCards[0]?.id;
      activeSimId = target;
      if (activeSimId) loadTabData(activeSimId);
    }
  });

  async function loadTabData(simId) {
    if (!simId) return;
    isCurrentTabLoading = true;
    try {
      const response = await apiClient.getSimInfo(simId);
      tabDataCache[simId] = response.data || response;
      tabDataCache = { ...tabDataCache };
    } catch (e) {
      console.error(`Failed to load SIM info for ${simId}:`, e);
      tabDataCache[simId] = null;
      tabDataCache = { ...tabDataCache };
    } finally {
      isCurrentTabLoading = false;
    }
  }

  function switchTab(simId) {
    if (activeSimId === simId) return;
    activeSimId = simId;
    if (!tabDataCache[simId]) loadTabData(simId);
  }

  function refreshCurrentTab() {
    if (activeSimId) loadTabData(activeSimId);
  }

  async function handleUpdatePhone(simId, phone) {
    const success = await simCardActions.updatePhoneNumber(simId, phone);
    if (success && tabDataCache[simId]) await loadTabData(simId);
    return success;
  }

  async function handleUpdateAlias(simId, alias) {
    const success = await simCardActions.updateAlias(simId, alias);
    if (success && tabDataCache[simId]) await loadTabData(simId);
    return success;
  }

  function getSimDisplayName(sim) {
    return sim.alias || sim.phone_number || `SIM ${sim.id.slice(-8)}`;
  }

  const activeSimCard = $derived($simCards.find(s => s.id === activeSimId));
  const activeSimInfo = $derived(tabDataCache[activeSimId]);
</script>

<div class="flex flex-col h-dvh w-screen bg-white dark:bg-zinc-900 font-sans">
  <!-- Header -->
  <header class="flex items-center gap-3 px-4 py-3 border-b border-gray-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 shadow-sm shrink-0">
    <button
      onclick={onBack}
      class="inline-flex items-center justify-center w-9 h-9 rounded-lg border border-gray-200 dark:border-zinc-700
             text-gray-600 dark:text-gray-300 hover:bg-gray-50 dark:hover:bg-zinc-800 transition"
      aria-label="Back"
    >
      <Icon icon="carbon:arrow-left" class="w-5 h-5" />
    </button>
    <div class="flex items-center gap-2">
      <Icon icon="carbon:sim-card" class="w-5 h-5 text-gray-500 dark:text-gray-400" />
      <h1 class="text-base font-semibold text-gray-800 dark:text-gray-100">{$t('sim_card_information')}</h1>
    </div>
  </header>

  <!-- Tabs -->
  <SimCardTabs
    simCards={$simCards}
    {activeSimId}
    onTabSwitch={switchTab}
    getDisplayName={getSimDisplayName}
  />

  <!-- Content -->
  <div class="flex-1 overflow-auto p-4 sm:p-6">
    <SimCardTabContent
      simCard={activeSimCard}
      simInfo={activeSimInfo}
      isLoading={isCurrentTabLoading}
      onRefresh={refreshCurrentTab}
      onUpdatePhone={(phone) => handleUpdatePhone(activeSimId, phone)}
      onUpdateAlias={(alias) => handleUpdateAlias(activeSimId, alias)}
      getDisplayName={getSimDisplayName}
    />
  </div>
</div>

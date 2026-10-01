<script>
  import { onMount } from 'svelte';
  import Icon from '@iconify/svelte';
  import CallLog from '../components/conversation/CallLog.svelte';
  import IncomingCallBanner from '../components/conversation/IncomingCallBanner.svelte';
  import { simCardActions } from '../stores/simcards.js';
  import { t } from '../js/i18n.js';

  let { onBack = () => {}, filterSimId = null } = $props();

  onMount(async () => {
    await simCardActions.loadAll();
  });
</script>

<div class="flex flex-col h-dvh w-screen bg-white dark:bg-zinc-900 font-sans">
  <IncomingCallBanner />

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
      <Icon icon="carbon:phone" class="w-5 h-5 text-gray-500 dark:text-gray-400" />
      <h1 class="text-base font-semibold text-gray-800 dark:text-gray-100">{$t('call_log_title')}</h1>
    </div>
  </header>

  <!-- Content -->
  <div class="flex-1 overflow-hidden">
    <CallLog {filterSimId} />
  </div>
</div>

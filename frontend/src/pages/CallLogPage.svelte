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
      class="inline-flex items-center justify-center w-9 h-9 rounded-lg border border-gray-200 dark:border-zinc-700
             text-gray-600 dark:text-gray-300 hover:bg-gray-50 dark:hover:bg-zinc-800 transition"
      aria-label="Back"
    >
      <Icon icon="carbon:arrow-left" class="w-5 h-5" />
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

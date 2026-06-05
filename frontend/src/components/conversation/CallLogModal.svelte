<script>
  import Modal from '../common/Modal.svelte';
  import CallLog from './CallLog.svelte';
  import Icon from '@iconify/svelte';

  let { isOpen = false, onClose = () => {}, filterSimId = null } = $props();
</script>

<Modal {isOpen} {onClose} maxWidth="max-w-lg">
  <div class="flex flex-col h-[80vh]">
    <!-- Modal header -->
    <div class="flex items-center justify-between px-5 py-4 border-b border-gray-200 dark:border-zinc-700 shrink-0">
      <div class="flex items-center gap-2">
        <Icon icon="carbon:phone" class="w-5 h-5 text-gray-700 dark:text-gray-200" />
        <h2 class="text-base font-semibold text-gray-900 dark:text-white">Voice Calls</h2>
      </div>
      <button
        onclick={onClose}
        class="w-8 h-8 rounded-lg flex items-center justify-center text-gray-500 hover:text-gray-800 dark:text-zinc-400 dark:hover:text-white hover:bg-gray-100 dark:hover:bg-zinc-800 transition"
        aria-label="Close"
      >
        <Icon icon="carbon:close" class="w-4 h-4" />
      </button>
    </div>

    <!-- Call log content -->
    <div class="flex-1 overflow-hidden">
      <CallLog {filterSimId} />
    </div>
  </div>
</Modal>

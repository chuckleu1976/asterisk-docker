<script>
  import ConversationList from "../conversation/ConversationList.svelte";

  let { onConversationSelect = () => {}, filterSimId = null } = $props();
</script>

<div
  class="h-full lg:h-dvh w-full lg:w-80 bg-white dark:bg-zinc-900 p-4 border-r border-gray-200 dark:border-zinc-700 flex flex-col"
>
  <div class="flex-1 overflow-hidden">
    <ConversationList onConversationSelect={onConversationSelect} {filterSimId} />
  </div>
</div>

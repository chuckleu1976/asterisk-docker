<script>
  import Icon from "@iconify/svelte";
  import { formatDate } from "../../js/dateFormat";
  import { fade, scale } from "svelte/transition";
  import { t } from "../../js/i18n.js";
  import {
    conversations,
    currentContact,
    changeCurrentConversation,
    conversationLoading,
  } from "../../stores/conversation";
  import { apiClient } from "../../js/api";
  import { simCards } from "../../stores/simcards";
  import { generateUUID } from "../../js/uuid";

  let { onConversationSelect = () => {}, filterSimId = null } = $props();

  // 鈹€鈹€ Tab state 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  let activeTab = $state("inbox");   // "inbox" | "sent"
  let messages = $state([]);
  let loading = $state(true);
  let searchValue = $state("");
  let searchFocused = $state(false);

  // Unread count from conversations store (for badge on Inbox tab)
  let unreadCount = $derived(
    $conversations.filter(c => c.sms_preview?.status === 0).length
  );

  // 鈹€鈹€ Fetch on tab change or SSE push 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  async function fetchMessages(tab) {
    loading = true;
    try {
      const res = await apiClient.getSmsByDirection(tab);
      messages = res.data?.data ?? [];
    } catch (e) {
      console.error("Failed to load messages:", e);
      messages = [];
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    // Re-fetch when tab changes; also when conversations store updates (SSE push).
    const _tab = activeTab;
    const _conv = $conversations;   // reactive dependency
    fetchMessages(_tab);
  });

  // 鈹€鈹€ Filtered list 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  let filtered = $derived(
    messages
      .filter(m => !filterSimId || m.sim_id === filterSimId)
      .filter(m =>
        searchValue.trim() === "" ||
        m.contact_name.toLowerCase().includes(searchValue.toLowerCase()) ||
        m.message.toLowerCase().includes(searchValue.toLowerCase())
      )
  );

  // 鈹€鈹€ SIM display name 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  function getSimName(simId) {
    if (!simId) return "";
    const sim = $simCards.find(s => s.id === simId);
    return sim?.alias || sim?.phone_number || simId.slice(-6);
  }

  // 鈹€鈹€ Avatar helpers 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  const AVATAR_COLORS = [
    "bg-blue-500", "bg-emerald-500", "bg-violet-500", "bg-amber-500",
    "bg-rose-500",  "bg-cyan-500",   "bg-indigo-500", "bg-teal-500",
  ];
  function avatarColor(name) {
    let n = 0;
    for (let i = 0; i < (name || "?").length; i++) n += name.charCodeAt(i);
    return AVATAR_COLORS[n % AVATAR_COLORS.length];
  }
  function initials(name) {
    if (!name) return "?";
    const parts = name.trim().split(/\s+/);
    return parts.length > 1
      ? (parts[0][0] + parts[1][0]).toUpperCase()
      : name.slice(0, 2).toUpperCase();
  }

  // 鈹€鈹€ Open conversation 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  function openMessage(msg) {
    changeCurrentConversation({ id: msg.contact_id, name: msg.contact_name });
    onConversationSelect();
  }

  // 鈹€鈹€ Compose new message 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€
  function compose() {
    const existing = $conversations.find(c => c.contact.new === true);
    if (existing) {
      changeCurrentConversation(existing.contact);
    } else {
      changeCurrentConversation({ id: generateUUID(), name: "New message", new: true });
    }
    onConversationSelect();
  }
</script>

<div class="flex flex-col h-full">

  <!-- 鈹€鈹€ Header 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ -->
  <div class="flex items-center justify-between mb-3">
    <h2 class="text-base font-semibold text-gray-800 dark:text-gray-200">{$t('messages')}</h2>
    <button
      class="p-2 rounded-lg bg-gray-800 dark:bg-gray-200 text-gray-100 dark:text-gray-900
             hover:bg-gray-700 dark:hover:bg-gray-300 transition-all active:scale-95"
      onclick={compose}
      title={$t('compose')}
    >
      <Icon icon="carbon:edit" class="w-4 h-4" />
    </button>
  </div>

  <!-- SIM filter banner -->
  {#if filterSimId}
    <div class="flex items-center gap-1.5 mb-2 px-2 py-1.5 rounded-lg
                bg-blue-50 dark:bg-blue-950/30 border border-blue-200 dark:border-blue-800/50">
      <Icon icon="carbon:filter" class="w-3.5 h-3.5 text-blue-500 flex-shrink-0" />
      <span class="text-[11px] text-blue-700 dark:text-blue-300 font-medium truncate flex-1">
        {$t('filter_by_sim', { sim: getSimName(filterSimId) })}
      </span>
    </div>
  {/if}

  <!-- 鈹€鈹€ Tabs 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ -->
  <div class="flex gap-1 mb-3 p-1 rounded-lg bg-gray-100 dark:bg-zinc-800">
    {#each [{ id: "inbox", label: $t('inbox'), icon: "carbon:email" },
            { id: "sent",  label: $t('sent'),  icon: "carbon:send-alt" }] as tab}
      <button
        class="relative flex-1 flex items-center justify-center gap-1.5 py-1.5 rounded-md text-xs font-medium
               transition-all duration-200
               {activeTab === tab.id
                 ? 'bg-white dark:bg-zinc-700 text-gray-900 dark:text-gray-100 shadow-sm'
                 : 'text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-300'}"
        onclick={() => { activeTab = tab.id; }}
      >
        <Icon icon={tab.icon} class="w-3.5 h-3.5" />
        {tab.label}
        {#if tab.id === "inbox" && unreadCount > 0}
          <span
            class="absolute -top-1 -right-1 min-w-[16px] h-4 px-1 rounded-full
                   bg-red-500 text-white text-[10px] font-bold flex items-center justify-center"
            transition:scale={{ duration: 150 }}
          >
            {unreadCount > 99 ? "99+" : unreadCount}
          </span>
        {/if}
      </button>
    {/each}
  </div>

  <!-- 鈹€鈹€ Search 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ -->
  <div class="mb-3">
    <div class="flex items-center gap-2 px-3 py-2 rounded-lg border transition-all
                bg-white dark:bg-zinc-900
                {searchFocused
                  ? 'border-gray-400 dark:border-zinc-500'
                  : 'border-gray-300 dark:border-zinc-600'}">
      <Icon icon="carbon:search" class="w-4 h-4 text-gray-400 flex-shrink-0" />
      <input
        type="text"
        bind:value={searchValue}
        onfocus={() => searchFocused = true}
        onblur={() => searchFocused = false}
        placeholder={activeTab === "inbox" ? $t('search_inbox') : $t('search_sent')}
        class="flex-1 bg-transparent text-sm text-gray-700 dark:text-gray-200
               placeholder-gray-400 dark:placeholder-gray-500 outline-none border-0"
      />
      {#if searchValue}
        <button onclick={() => searchValue = ""}>
          <Icon icon="carbon:close" class="w-3.5 h-3.5 text-gray-400" />
        </button>
      {/if}
    </div>
  </div>

  <!-- 鈹€鈹€ Message list 鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€鈹€ -->
  <div class="flex-1 overflow-y-auto scrollbar-thin -mx-1 px-1">
    {#if loading || $conversationLoading}
      <!-- Skeleton -->
      <div class="space-y-1">
        {#each Array(7) as _}
          <div class="flex items-center gap-3 px-2 py-2.5 rounded-lg animate-pulse">
            <div class="w-9 h-9 rounded-full bg-gray-200 dark:bg-zinc-700 flex-shrink-0"></div>
            <div class="flex-1 space-y-1.5">
              <div class="flex justify-between">
                <div class="h-3 w-24 bg-gray-200 dark:bg-zinc-700 rounded"></div>
                <div class="h-3 w-10 bg-gray-200 dark:bg-zinc-700 rounded"></div>
              </div>
              <div class="h-2.5 w-4/5 bg-gray-200 dark:bg-zinc-700 rounded"></div>
            </div>
          </div>
        {/each}
      </div>

    {:else if filtered.length === 0}
      <!-- Empty state -->
      <div class="flex flex-col items-center justify-center py-14 text-center" transition:fade={{ duration: 200 }}>
        <div class="w-14 h-14 rounded-full bg-gray-100 dark:bg-zinc-800 flex items-center justify-center mb-3">
          <Icon
            icon={activeTab === "inbox" ? "carbon:email" : "carbon:send-alt"}
            class="w-7 h-7 text-gray-400 dark:text-gray-500"
          />
        </div>
        <p class="text-sm font-medium text-gray-700 dark:text-gray-300">
          {searchValue
            ? $t('no_results')
            : filterSimId
              ? activeTab === "inbox" ? $t('no_messages_for_sim') : $t('nothing_sent_for_sim')
              : activeTab === "inbox" ? $t('no_messages') : $t('nothing_sent')}
        </p>
        {#if !searchValue && activeTab === "sent"}
          <button
            onclick={compose}
            class="mt-4 px-4 py-2 bg-gray-800 dark:bg-gray-200 text-gray-100 dark:text-gray-900
                   rounded-lg text-sm font-medium flex items-center gap-2 transition-all active:scale-95"
          >
            <Icon icon="carbon:edit" class="w-4 h-4" /> {$t('compose')}
          </button>
        {/if}
      </div>

    {:else}
      <div class="space-y-0.5" transition:fade={{ duration: 150 }}>
        {#each filtered as msg (msg.id)}
          {@const isUnread = activeTab === "inbox" && msg.status === 0}
          {@const isSelected = $currentContact?.id === msg.contact_id}
          <button
            class="w-full text-left flex items-center gap-3 px-2 py-2.5 rounded-lg
                   transition-all duration-150 group
                   {isSelected
                     ? 'bg-gray-100 dark:bg-zinc-800'
                     : isUnread
                       ? 'bg-blue-50/60 dark:bg-blue-950/20 hover:bg-blue-50 dark:hover:bg-blue-950/30'
                       : 'hover:bg-gray-50 dark:hover:bg-zinc-800/60'}"
            onclick={() => openMessage(msg)}
          >
            <!-- Avatar -->
            <div class="flex-shrink-0 w-9 h-9 rounded-full {avatarColor(msg.contact_name)}
                        flex items-center justify-center text-white text-xs font-semibold">
              {initials(msg.contact_name)}
            </div>

            <!-- Content -->
            <div class="flex-1 min-w-0">
              <div class="flex items-center justify-between gap-1 mb-0.5">
                <span class="text-[13px] truncate
                             {isUnread ? 'font-semibold text-gray-900 dark:text-gray-100'
                                       : 'font-medium text-gray-700 dark:text-gray-300'}">
                  {msg.contact_name}
                </span>
                <span class="text-[11px] flex-shrink-0
                             {isUnread ? 'font-semibold text-blue-600 dark:text-blue-400'
                                       : 'text-gray-400 dark:text-gray-500'}">
                  {formatDate(msg.timestamp)}
                </span>
              </div>

              <div class="flex items-center gap-1.5">
                <!-- SIM badge -->
                <span class="inline-block px-1.5 py-0.5 text-[10px] rounded font-medium flex-shrink-0
                             bg-gray-200 dark:bg-zinc-700 text-gray-500 dark:text-gray-400">
                  {getSimName(msg.sim_id)}
                </span>
                <!-- Snippet -->
                <p class="text-[12px] truncate
                          {isUnread ? 'text-gray-700 dark:text-gray-300'
                                    : 'text-gray-400 dark:text-gray-500'}">
                  {msg.message}
                </p>
                <!-- Unread dot -->
                {#if isUnread}
                  <span class="flex-shrink-0 w-2 h-2 rounded-full bg-blue-500 ml-auto"></span>
                {/if}
              </div>
            </div>
          </button>
        {/each}
      </div>
    {/if}
  </div>

</div>

<style>
  .scrollbar-thin { scrollbar-width: thin; scrollbar-color: rgb(156 163 175) transparent; }
  .scrollbar-thin::-webkit-scrollbar { width: 4px; }
  .scrollbar-thin::-webkit-scrollbar-track { background: transparent; }
  .scrollbar-thin::-webkit-scrollbar-thumb { background: #d1d5db; border-radius: 4px; }
</style>

<script>
  import { fly } from 'svelte/transition';
  import Icon from '@iconify/svelte';
  import { incomingCall, activeCall, callActions } from '../../stores/calls.js';

  let call = $derived($incomingCall);
  let active = $derived($activeCall);

  // Show banner when there's an incoming OR active call
  let show = $derived(call !== null || active !== null);

  let busy = $state(false);

  async function handleAnswer() {
    if (!call) return;
    busy = true;
    try { await callActions.answer(call.sim_id); }
    catch (e) { console.error('Answer failed:', e); }
    finally { busy = false; }
  }

  async function handleHangup() {
    const target = call ?? active;
    if (!target) return;
    busy = true;
    try { await callActions.hangup(target.sim_id); }
    catch (e) { console.error('Hangup failed:', e); }
    finally { busy = false; }
  }

  function formatPhone(phone) {
    return phone || 'Unknown number';
  }

  function statusLabel() {
    if (call) return 'Incoming call';
    if (active?.direction === 'outbound') return 'Calling…';
    return 'Call in progress';
  }
</script>

{#if show}
  <div
    class="fixed top-4 left-1/2 -translate-x-1/2 z-50 w-full max-w-sm px-4"
    transition:fly={{ y: -24, duration: 250 }}
  >
    <div class="rounded-2xl shadow-2xl border border-white/20 bg-zinc-900/95 backdrop-blur-md text-white px-5 py-4 flex items-center gap-4">
      <!-- Pulsing avatar -->
      <div class="relative shrink-0">
        <div class="w-12 h-12 rounded-full bg-green-500/20 flex items-center justify-center">
          <Icon icon="carbon:phone" class="w-6 h-6 text-green-400" />
        </div>
        {#if call}
          <span class="absolute inset-0 rounded-full animate-ping bg-green-400/30"></span>
        {/if}
      </div>

      <!-- Info -->
      <div class="flex-1 min-w-0">
        <p class="text-xs text-zinc-400 leading-none mb-0.5">{statusLabel()}</p>
        <p class="font-semibold truncate leading-snug">{formatPhone(call?.phone ?? active?.phone)}</p>
        {#if call?.sim_id || active?.sim_id}
          <p class="text-xs text-zinc-500 truncate">{call?.sim_id ?? active?.sim_id}</p>
        {/if}
      </div>

      <!-- Actions -->
      <div class="flex items-center gap-2 shrink-0">
        {#if call}
          <!-- Answer -->
          <button
            onclick={handleAnswer}
            disabled={busy}
            class="w-11 h-11 rounded-full bg-green-500 hover:bg-green-400 active:scale-95 transition flex items-center justify-center disabled:opacity-50"
            aria-label="Answer call"
          >
            <Icon icon="carbon:phone" class="w-5 h-5 text-white" />
          </button>
        {/if}
        <!-- Hang up / reject -->
        <button
          onclick={handleHangup}
          disabled={busy}
          class="w-11 h-11 rounded-full bg-red-500 hover:bg-red-400 active:scale-95 transition flex items-center justify-center disabled:opacity-50"
          aria-label={call ? 'Reject call' : 'Hang up'}
        >
          <Icon icon="carbon:phone-off" class="w-5 h-5 text-white rotate-135" />
        </button>
      </div>
    </div>
  </div>
{/if}

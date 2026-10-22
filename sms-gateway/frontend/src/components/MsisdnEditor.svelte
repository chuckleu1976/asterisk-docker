<script>
  import Icon from '@iconify/svelte';
  import { apiClient } from '../js/api.js';

  let { rental, value = $bindable('') } = $props();
  let info = $state(null);
  let loading = $state(false);
  let saving = $state(false);
  let error = $state('');
  let saved = $state(false);
  const canSave = $derived(info?.permissions?.includes('esim:msisdn') ?? false);
  const onCard = $derived(String(info?.sim_msisdn ?? '').trim());
  const operatorOwned = $derived(onCard.length > 0);
  const valid = $derived(value === '' || /^\+?[0-9]{1,20}$/.test(value));

  function errorText(problem) {
    if (problem?.status === 409) return 'Card changed. Refresh before saving.';
    return problem?.data?.error ?? problem?.message ?? 'Unable to save configured MSISDN';
  }

  $effect(() => {
    const id = rental?.id;
    let cancelled = false;
    info = null;
    saved = false;
    error = '';
    if (id) {
      loading = true;
      apiClient.configuredMsisdn(id).then((response) => {
        if (cancelled) return;
        info = response.data;
        if (!value && !String(info.sim_msisdn ?? '').trim()) value = info.operator_msisdn ?? '';
      }).catch((problem) => {
        if (!cancelled) error = errorText(problem);
      }).finally(() => {
        if (!cancelled) loading = false;
      });
    }
    return () => { cancelled = true; };
  });

  async function save(number = value) {
    if (!info || operatorOwned || !canSave || (number !== '' && !/^\+?[0-9]{1,20}$/.test(number)) || saving) return;
    saving = true;
    error = '';
    saved = false;
    const id = rental.id;
    try {
      const response = await apiClient.saveConfiguredMsisdn(id, {
        phone_number: number,
        presence_generation: info.presence_generation,
        identity_generation: info.identity_generation,
      });
      if (id !== rental?.id) return;
      info = response.data;
      value = info.operator_msisdn ?? '';
      saved = true;
    } catch (problem) {
      error = errorText(problem);
    } finally {
      saving = false;
    }
  }
</script>

<div class="min-w-0 space-y-2">
  {#if loading}
    <p class="text-xs text-gray-400">Loading</p>
  {:else if operatorOwned}
    <p class="break-all font-mono text-sm text-gray-800 dark:text-gray-100">{onCard}</p>
    <p class="text-xs text-gray-500">On card (operator)</p>
  {:else}
    <label class="grid min-w-0 gap-1 text-sm">
      Configured MSISDN (database)
      <span class="flex min-w-0 items-center gap-1">
        <input type="tel" aria-label="Configured MSISDN" autocomplete="off" maxlength="21"
          class="min-w-0 w-full rounded border bg-white px-2 py-1 text-gray-900 dark:bg-zinc-800 dark:text-gray-100"
          bind:value oninput={() => { saved = false; }} disabled={loading || saving || !canSave}
          onkeydown={(event) => { if (event.key === 'Enter') { event.preventDefault(); save(); } }} />
        <button type="button" class="inline-flex h-8 shrink-0 items-center justify-center gap-1 rounded border px-2 disabled:opacity-40"
          title="Save configured MSISDN to database" aria-label="Save configured MSISDN" onclick={() => save()}
          disabled={loading || saving || !canSave || !valid}>
          <Icon icon="carbon:save" width="18" />
          Save
        </button>
        <button type="button" class="flex h-8 w-8 shrink-0 items-center justify-center rounded border disabled:opacity-40"
          title="Clear configured MSISDN" aria-label="Clear configured MSISDN" onclick={() => save('')}
          disabled={loading || saving || !canSave || !info?.operator_msisdn}>
          <Icon icon="carbon:erase" width="18" />
        </button>
      </span>
    </label>
    {#if !valid}<p class="text-xs text-red-600">Use digits with an optional leading +.</p>{/if}
    {#if saving}<p role="status" class="text-xs text-gray-500">Saving</p>{/if}
    {#if saved}<p role="status" class="text-xs text-green-700">Saved to database</p>{/if}
  {/if}
  {#if error}<p role="alert" class="break-words text-xs text-red-600">{error}</p>{/if}
</div>
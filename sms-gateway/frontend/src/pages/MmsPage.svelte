<script>
  import { onMount, onDestroy } from 'svelte';
  import Icon from '@iconify/svelte';
  import { apiClient } from '../js/api.js';
  import { t } from '../js/i18n.js';
  import { displayPort } from '../js/modem.js';
  import Modal from '../components/common/Modal.svelte';

  let { onBack = () => {}, initialSimId = null } = $props();

  // ── SIM list (active modems) ────────────────────────────────────────────
  let sims = $state([]);
  let simsLoading = $state(true);
  let selectedSimId = $state('');

  // ── Compose form ────────────────────────────────────────────────────────
  let toNumber = $state('');
  let subject = $state('');
  /** @type {{filename: string, contentType: string, base64: string, size: number}[]} */
  let attachments = $state([]);
  let sending = $state(false);
  let sendError = $state('');
  let sendSuccess = $state('');
  let fileInputEl = $state(null);

  // ── MMS profile (APN/MMSC/proxy) ────────────────────────────────────────
  let profileApn = $state('');
  let profileMmsc = $state('');
  let profileProxyHost = $state('');
  let profileProxyPort = $state('');
  let profileFtpApn = $state('');
  let profileSendMode = $state('modem_direct_attachment_upload');
  let profileLoading = $state(false);
  let profileSaving = $state(false);
  let profileError = $state('');
  let profileSuccess = $state('');

  // ── History ──────────────────────────────────────────────────────────────
  let history = $state([]);
  let historyTotal = $state(0);
  let historyPage = $state(1);
  let historyPerPage = 20;
  let historyLoading = $state(true);
  let historyError = $state('');
  let detailJob = $state(null);
  let detailAttachments = $state([]);
  let detailOpen = $state(false);

  // ── Inbox: detected notifications + fetched content (subject/from/parts) ──
  let inbox = $state([]);
  let inboxTotal = $state(0);
  let inboxPage = $state(1);
  let inboxPerPage = 20;
  let inboxLoading = $state(true);
  let inboxError = $state('');
  let inboxDetailItem = $state(null);
  let inboxDetailParts = $state([]);
  let inboxDetailLoading = $state(false);
  let inboxDetailOpen = $state(false);
  let inboxPartPreviewOpen = $state(false);
  let inboxPartPreviewTitle = $state('');
  let inboxPartPreviewText = $state('');

  let pollTimer = null;

  const totalPages = $derived(Math.max(1, Math.ceil(historyTotal / historyPerPage)));
  const inboxTotalPages = $derived(Math.max(1, Math.ceil(inboxTotal / inboxPerPage)));
  const totalAttachmentBytes = $derived(attachments.reduce((sum, a) => sum + a.size, 0));

  async function loadSims() {
    simsLoading = true;
    try {
      const response = await apiClient.getAllSimsInfo();
      const infos = Array.isArray(response) ? response : (response?.data ?? []);
      sims = infos;
      if (!selectedSimId && sims.length > 0) {
        const preferred = initialSimId && sims.find((sim) => sim.sim_id === initialSimId);
        selectedSimId = preferred ? preferred.sim_id : sims[0].sim_id;
      } else if (selectedSimId && !sims.some((sim) => sim.sim_id === selectedSimId) && sims.length > 0) {
        const preferred = initialSimId && sims.find((sim) => sim.sim_id === initialSimId);
        selectedSimId = preferred ? preferred.sim_id : sims[0].sim_id;
      }
    } catch (e) {
      console.error('Failed to load SIM list:', e);
      sims = [];
    } finally {
      simsLoading = false;
    }
  }

  async function loadProfile(simId) {
    if (!simId) return;
    profileLoading = true;
    profileError = '';
    profileSuccess = '';
    try {
      const res = await apiClient.getMmsProfile(simId);
      const profile = res?.data ?? res ?? {};
      profileApn = profile.mms_apn ?? '';
      profileMmsc = profile.mms_mmsc ?? '';
      profileProxyHost = profile.mms_proxy_host ?? '';
      profileProxyPort = profile.mms_proxy_port != null ? String(profile.mms_proxy_port) : '';
      profileFtpApn = profile.ftp_apn ?? '';
      profileSendMode = profile.mms_send_mode || 'modem_direct_attachment_upload';
    } catch (e) {
      console.error('Failed to load MMS profile:', e);
      profileError = $t('err_profile_load_failed');
      profileApn = '';
      profileMmsc = '';
      profileProxyHost = '';
      profileProxyPort = '';
      profileFtpApn = '';
      profileSendMode = 'modem_direct_attachment_upload';
    } finally {
      profileLoading = false;
    }
  }

  async function saveProfile() {
    if (!selectedSimId) {
      profileError = $t('err_no_sim_selected');
      return;
    }
    profileSaving = true;
    profileError = '';
    profileSuccess = '';
    try {
      const payload = {
        apn: String(profileApn ?? '').trim() || null,
        mmsc: String(profileMmsc ?? '').trim() || null,
        proxy_host: String(profileProxyHost ?? '').trim() || null,
        proxy_port: String(profileProxyPort ?? '').trim() ? Number(String(profileProxyPort).trim()) : null,
        mms_send_mode: profileSendMode || 'modem_direct_attachment_upload',
        ftp_apn: String(profileFtpApn ?? '').trim() || null,
      };
      await apiClient.setMmsProfile(selectedSimId, payload);
      profileSuccess = $t('msg_profile_saved');
    } catch (e) {
      console.error('Failed to save MMS profile:', e);
      profileError = e?.data?.error ?? e?.message ?? $t('err_profile_save_failed');
    } finally {
      profileSaving = false;
    }
  }

  async function handleSendModeChange(event) {
    profileSendMode = event.currentTarget.value;
    await saveProfile();
    if (!profileError) {
      await loadProfile(selectedSimId);
    }
  }

  function fileToBase64(file) {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => {
        const result = /** @type {string} */ (reader.result);
        const base64 = result.includes(',') ? result.split(',')[1] : result;
        resolve(base64);
      };
      reader.onerror = () => reject(reader.error);
      reader.readAsDataURL(file);
    });
  }

  async function handleFilesSelected(e) {
    const files = Array.from(e.target?.files ?? []);
    for (const file of files) {
      try {
        const base64 = await fileToBase64(file);
        attachments.push({
          filename: file.name,
          contentType: file.type || 'application/octet-stream',
          base64,
          size: file.size,
        });
      } catch (err) {
        console.error('Failed to read file:', file.name, err);
      }
    }
    // Reset the input so the same file can be re-selected later.
    if (fileInputEl) fileInputEl.value = '';
  }

  function removeAttachment(index) {
    attachments.splice(index, 1);
  }

  function formatBytes(bytes) {
    if (!bytes) return '0 B';
    const units = ['B', 'KB', 'MB', 'GB'];
    let i = 0;
    let value = bytes;
    while (value >= 1024 && i < units.length - 1) {
      value /= 1024;
      i++;
    }
    return `${value.toFixed(value < 10 && i > 0 ? 1 : 0)} ${units[i]}`;
  }

  async function handleSend(e) {
    e.preventDefault();
    sendError = '';
    sendSuccess = '';

    if (!selectedSimId) {
      sendError = $t('err_no_sim_selected');
      return;
    }
    if (!toNumber.trim()) {
      sendError = $t('err_to_required');
      return;
    }

    sending = true;
    try {
      const payload = {
        sim_id: selectedSimId,
        to: toNumber.trim(),
        subject: subject.trim() || null,
        attachments: attachments.map((a) => ({
          filename: a.filename,
          content_type: a.contentType,
          base64: a.base64,
        })),
      };
      const res = await apiClient.sendMms(payload);
      const id = res?.data?.id ?? res?.id ?? '';
      sendSuccess = $t('msg_mms_queued', { id });
      toNumber = '';
      subject = '';
      attachments = [];
      await loadHistory(1);
    } catch (e) {
      console.error('Failed to send MMS:', e);
      sendError = e?.data?.error ?? e?.message ?? $t('err_mms_send_failed');
    } finally {
      sending = false;
    }
  }

  async function loadHistory(page = historyPage) {
    historyLoading = true;
    historyError = '';
    try {
      const res = await apiClient.getMmsPaginated(page, historyPerPage);
      const body = res?.data ?? res ?? {};
      history = body.data ?? [];
      historyTotal = body.total ?? 0;
      historyPage = body.page ?? page;
    } catch (e) {
      console.error('Failed to load MMS history:', e);
      historyError = e?.message ?? 'Failed to load MMS history';
    } finally {
      historyLoading = false;
    }
  }

  async function openDetail(job) {
    try {
      const res = await apiClient.getMmsDetail(job.id);
      const body = res?.data ?? res ?? {};
      detailJob = body.job ?? job;
      detailAttachments = body.attachments ?? [];
    } catch (e) {
      console.error('Failed to load MMS detail:', e);
      detailJob = job;
      detailAttachments = [];
    }
    detailOpen = true;
  }

  function closeDetail() {
    detailOpen = false;
    detailJob = null;
    detailAttachments = [];
  }

  function statusBadgeClass(status) {
    switch (status) {
      case 'sent':
        return 'bg-green-100 text-green-700 dark:bg-green-900/30 dark:text-green-300';
      case 'failed':
      case 'timeout':
        return 'bg-red-100 text-red-700 dark:bg-red-900/30 dark:text-red-300';
      case 'sending':
        return 'bg-blue-100 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300';
      default:
        return 'bg-gray-100 text-gray-700 dark:bg-gray-700/50 dark:text-gray-300';
    }
  }

  function statusLabel(status) {
    switch (status) {
      case 'queued': return $t('mms_status_queued');
      case 'sending': return $t('mms_status_sending');
      case 'sent': return $t('mms_status_sent');
      case 'failed': return $t('mms_status_failed');
      case 'timeout': return $t('mms_status_timeout');
      default: return status;
    }
  }

  function formatDate(ts) {
    if (!ts) return '-';
    const iso = typeof ts === 'string' && !ts.endsWith('Z') && !ts.includes('+') ? `${ts}Z` : ts;
    const d = new Date(iso);
    return isNaN(d.getTime()) ? String(ts) : d.toLocaleString();
  }

  function hasPendingJobs() {
    return history.some((job) => job.status === 'queued' || job.status === 'sending');
  }

  async function loadInbox(page = inboxPage) {
    inboxLoading = true;
    inboxError = '';
    try {
      const res = await apiClient.getMmsInboxPaginated(page, inboxPerPage);
      const body = res?.data ?? res ?? {};
      inbox = body.data ?? [];
      inboxTotal = body.total ?? 0;
      inboxPage = body.page ?? page;
    } catch (e) {
      console.error('Failed to load MMS inbox:', e);
      inboxError = e?.message ?? 'Failed to load MMS inbox';
    } finally {
      inboxLoading = false;
    }
  }

  async function openInboxDetail(item) {
    inboxDetailItem = item;
    inboxDetailParts = [];
    inboxDetailOpen = true;
    inboxDetailLoading = true;
    try {
      const res = await apiClient.getMmsInboxDetail(item.id);
      const body = res?.data ?? res ?? {};
      inboxDetailItem = body.notification ?? item;
      inboxDetailParts = body.parts ?? [];
    } catch (e) {
      console.error('Failed to load MMS inbox detail:', e);
    } finally {
      inboxDetailLoading = false;
    }
  }

  function closeInboxDetail() {
    inboxDetailOpen = false;
    inboxDetailItem = null;
    inboxDetailParts = [];
    inboxPartPreviewOpen = false;
    inboxPartPreviewTitle = '';
    inboxPartPreviewText = '';
  }

  function isLikelyTextPart(part, blob) {
    const contentType = String(part?.content_type || blob?.type || '').toLowerCase();
    const filename = String(part?.filename || '').toLowerCase();
    return contentType.startsWith('text/')
      || contentType.includes('smil')
      || filename.includes('text')
      || filename.includes('smil');
  }

  async function viewInboxPart(part) {
    if (!inboxDetailItem) return;
    try {
      const blob = await apiClient.getMmsInboxPartBlob(inboxDetailItem.id, part.id);
      if (isLikelyTextPart(part, blob)) {
        inboxPartPreviewTitle = part.filename || part.content_type || 'text/plain';
        inboxPartPreviewText = await blob.text();
        inboxPartPreviewOpen = true;
        return;
      }
      const url = URL.createObjectURL(blob);
      window.open(url, '_blank');
      // Give the new tab time to load the resource before revoking it.
      setTimeout(() => URL.revokeObjectURL(url), 60000);
    } catch (e) {
      console.error('Failed to load MMS part:', e);
    }
  }

  function inboxStatusBadgeClass(status) {
    switch (status) {
      case 'fetched':
        return 'bg-green-100 text-green-700 dark:bg-green-900/30 dark:text-green-300';
      case 'delivery_report':
        return 'bg-blue-100 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300';
      case 'failed':
      case 'expired':
        return 'bg-red-100 text-red-700 dark:bg-red-900/30 dark:text-red-300';
      case 'fetching':
        return 'bg-blue-100 text-blue-700 dark:bg-blue-900/30 dark:text-blue-300';
      default:
        return 'bg-gray-100 text-gray-700 dark:bg-gray-700/50 dark:text-gray-300';
    }
  }

  function inboxStatusLabel(status) {
    switch (status) {
      case 'notified': return $t('mms_inbox_status_notified');
      case 'fetching': return $t('mms_inbox_status_fetching');
      case 'fetched': return $t('mms_inbox_status_fetched');
      case 'failed': return $t('mms_inbox_status_failed');
      case 'expired': return $t('mms_inbox_status_expired');
      case 'delivery_report': return $t('mms_inbox_status_delivery_report');
      default: return status;
    }
  }

  function formatMmsSize(bytes) {
    if (bytes === null || bytes === undefined) return '-';
    return formatBytes(bytes);
  }

  function normalizeMmsSender(value) {
    const raw = String(value ?? '').trim();
    if (!raw) return '';

    // Typical format: +8618126101015/TYPE=PLMN
    const noType = raw.split('/')[0].trim();
    const digits = noType.replace(/\D/g, '');

    // Mainland China mobile numbers: drop country code when present.
    if (digits.length === 13 && digits.startsWith('86') && digits[2] === '1') {
      return digits.slice(2);
    }
    if (digits.length === 11 && digits.startsWith('1')) {
      return digits;
    }

    return noType || raw;
  }

  function mmsSenderDisplay(item) {
    const from = normalizeMmsSender(item?.from_address);
    if (from) return from;
    return normalizeMmsSender(item?.sender) || '-';
  }

  function mmsReceiverDisplay(item) {
    const sim = sims.find((candidate) => candidate.sim_id === item?.sim_id);
    return sim?.phone_number || displayPort(sim?.com_port) || item?.sim_id || '-';
  }

  $effect(() => {
    if (selectedSimId) {
      loadProfile(selectedSimId);
    }
  });

  onMount(() => {
    loadSims();
    loadHistory(1);
    loadInbox(1);
    // Light polling keeps outgoing status and newly received MMS visible without a manual refresh.
    pollTimer = setInterval(() => {
      if (hasPendingJobs()) {
        loadHistory(historyPage);
      }
      if (!inboxLoading) {
        loadInbox(inboxPage);
      }
    }, 5000);
  });

  onDestroy(() => {
    if (pollTimer) clearInterval(pollTimer);
  });
</script>

<div class="h-dvh overflow-y-auto bg-gray-50 dark:bg-gray-900 text-gray-900 dark:text-gray-100">
  <div class="sticky top-0 z-10 border-b border-gray-200 dark:border-gray-700 bg-white/90 dark:bg-gray-800/90 backdrop-blur">
    <div class="flex items-center justify-between px-4 py-3">
      <div class="flex items-center gap-3">
        <button
          onclick={onBack}
          class="inline-flex items-center gap-1.5 h-9 px-2.5 rounded-full bg-blue-600 text-white
                 shadow-sm shadow-blue-600/30 hover:bg-blue-700 transition"
          aria-label="Back"
        >
          <Icon icon="carbon:arrow-left" class="h-4 w-4" />
          <span class="text-xs font-semibold">{$t('btn_back')}</span>
        </button>
        <div class="flex items-center gap-2">
          <Icon icon="carbon:image" class="h-5 w-5 text-gray-500 dark:text-gray-400" />
          <h1 class="text-lg font-semibold">{$t('mms_page_title')}</h1>
        </div>
      </div>
      <button
        onclick={() => { loadHistory(historyPage); loadInbox(inboxPage); }}
        class="inline-flex items-center gap-2 rounded-lg bg-blue-600 px-3 py-1.5 text-sm text-white transition-colors hover:bg-blue-700"
      >
        <Icon icon="carbon:renew" class="h-4 w-4" />
        {$t('btn_refresh')}
      </button>
    </div>
  </div>

  <div class="mx-auto grid max-w-7xl grid-cols-1 gap-6 p-4 xl:grid-cols-[380px,1fr]">
    <!-- ── Left column: compose + profile ─────────────────────────────────── -->
    <div class="space-y-6">
      <section class="overflow-hidden rounded-xl border border-gray-200 bg-white shadow-sm dark:border-gray-700 dark:bg-gray-800">
        <div class="border-b border-gray-200 px-4 py-3 dark:border-gray-700">
          <div class="font-semibold">{$t('mms_compose_title')}</div>
        </div>
        <form onsubmit={handleSend} class="space-y-4 p-4">
          <div>
            <label for="mms-sim" class="mb-1.5 block text-xs font-medium text-gray-700 dark:text-gray-300">
              {$t('mms_sim_label')}
            </label>
            {#if simsLoading}
              <div class="h-9 animate-pulse rounded-lg bg-gray-100 dark:bg-gray-700"></div>
            {:else if sims.length === 0}
              <p class="text-xs text-amber-600 dark:text-amber-400">{$t('mms_no_active_sims')}</p>
            {:else}
              <select
                id="mms-sim"
                bind:value={selectedSimId}
                class="w-full rounded-lg border border-gray-300 bg-white px-3 py-2 text-sm text-gray-900
                       focus:border-transparent focus:outline-none focus:ring-2 focus:ring-blue-500
                       dark:border-zinc-700 dark:bg-zinc-950 dark:text-gray-100"
              >
                <option value="" disabled>{$t('mms_select_sim')}</option>
                {#each sims as sim}
                  <option value={sim.sim_id}>
                    {displayPort(sim.com_port) || sim.sim_id} {sim.phone_number ? `— ${sim.phone_number}` : ''}
                  </option>
                {/each}
              </select>
            {/if}
          </div>

          <div>
            <label for="mms-to" class="mb-1.5 block text-xs font-medium text-gray-700 dark:text-gray-300">
              {$t('mms_to_label')}
            </label>
            <input
              id="mms-to"
              type="tel"
              bind:value={toNumber}
              placeholder={$t('mms_to_ph')}
              disabled={sending}
              class="w-full rounded-lg border border-gray-300 bg-white px-3 py-2 text-sm text-gray-900
                     placeholder-gray-400 focus:border-transparent focus:outline-none focus:ring-2 focus:ring-blue-500
                     disabled:cursor-not-allowed disabled:opacity-60
                     dark:border-zinc-700 dark:bg-zinc-950 dark:text-gray-100 dark:placeholder-gray-600"
            />
          </div>

          <div>
            <label for="mms-subject" class="mb-1.5 block text-xs font-medium text-gray-700 dark:text-gray-300">
              {$t('mms_subject_label')}
            </label>
            <input
              id="mms-subject"
              type="text"
              bind:value={subject}
              placeholder={$t('mms_subject_ph')}
              disabled={sending}
              class="w-full rounded-lg border border-gray-300 bg-white px-3 py-2 text-sm text-gray-900
                     placeholder-gray-400 focus:border-transparent focus:outline-none focus:ring-2 focus:ring-blue-500
                     disabled:cursor-not-allowed disabled:opacity-60
                     dark:border-zinc-700 dark:bg-zinc-950 dark:text-gray-100 dark:placeholder-gray-600"
            />
          </div>

          <div>
            <div class="mb-1.5 flex items-center justify-between">
              <span class="text-xs font-medium text-gray-700 dark:text-gray-300">{$t('mms_attachments_label')}</span>
              {#if attachments.length > 0}
                <span class="text-xs text-gray-400 dark:text-gray-500">{formatBytes(totalAttachmentBytes)}</span>
              {/if}
            </div>
            <input
              bind:this={fileInputEl}
              type="file"
              multiple
              onchange={handleFilesSelected}
              disabled={sending}
              class="hidden"
              id="mms-file-input"
            />
            <label
              for="mms-file-input"
              class="inline-flex cursor-pointer items-center gap-1.5 rounded-lg border border-gray-200 px-3 py-1.5
                     text-xs font-medium text-gray-600 transition-colors hover:bg-gray-50
                     dark:border-zinc-700 dark:text-gray-300 dark:hover:bg-zinc-800"
            >
              <Icon icon="carbon:attachment" class="h-4 w-4" />
              {$t('mms_add_files')}
            </label>

            {#if attachments.length > 0}
              <ul class="mt-2 space-y-1.5">
                {#each attachments as att, i}
                  <li class="flex items-center justify-between rounded-lg border border-gray-100 bg-gray-50 px-2.5 py-1.5 text-xs dark:border-zinc-700 dark:bg-zinc-900">
                    <span class="truncate pr-2" title={att.filename}>{att.filename}</span>
                    <div class="flex shrink-0 items-center gap-2">
                      <span class="text-gray-400 dark:text-gray-500">{formatBytes(att.size)}</span>
                      <button
                        type="button"
                        onclick={() => removeAttachment(i)}
                        class="text-red-500 hover:text-red-600"
                        aria-label={$t('mms_remove_file')}
                      >
                        <Icon icon="carbon:close" class="h-3.5 w-3.5" />
                      </button>
                    </div>
                  </li>
                {/each}
              </ul>
            {:else}
              <p class="mt-2 text-xs text-gray-400 dark:text-gray-500">{$t('mms_no_files')}</p>
            {/if}
          </div>

          {#if sendError}
            <div class="flex items-start gap-2 rounded-lg bg-red-50 p-3 text-xs text-red-600 dark:bg-red-900/20 dark:text-red-400">
              <Icon icon="carbon:warning" class="mt-0.5 h-4 w-4 shrink-0" />
              <span>{sendError}</span>
            </div>
          {/if}
          {#if sendSuccess}
            <div class="flex items-start gap-2 rounded-lg bg-green-50 p-3 text-xs text-green-600 dark:bg-green-900/20 dark:text-green-400">
              <Icon icon="carbon:checkmark" class="mt-0.5 h-4 w-4 shrink-0" />
              <span>{sendSuccess}</span>
            </div>
          {/if}

          <button
            type="submit"
            disabled={sending || sims.length === 0}
            class="inline-flex w-full items-center justify-center gap-2 rounded-lg bg-blue-600 px-4 py-2
                   text-sm font-medium text-white transition-colors hover:bg-blue-700
                   disabled:cursor-not-allowed disabled:opacity-60"
          >
            {#if sending}
              <Icon icon="carbon:loading" class="h-4 w-4 animate-spin" />
              {$t('mms_sending')}
            {:else}
              <Icon icon="carbon:send" class="h-4 w-4" />
              {$t('btn_send_mms')}
            {/if}
          </button>
        </form>
      </section>

      <section class="overflow-hidden rounded-xl border border-gray-200 bg-white shadow-sm dark:border-gray-700 dark:bg-gray-800">
        <div class="border-b border-gray-200 px-4 py-3 dark:border-gray-700">
          <div class="font-semibold">{$t('mms_profile_title')}</div>
          <div class="text-xs text-gray-500 dark:text-gray-400">{$t('mms_profile_desc')}</div>
        </div>
        <div class="space-y-3 p-4">
          {#if profileLoading}
            <div class="space-y-2">
              <div class="h-8 animate-pulse rounded-lg bg-gray-100 dark:bg-gray-700"></div>
              <div class="h-8 animate-pulse rounded-lg bg-gray-100 dark:bg-gray-700"></div>
            </div>
          {:else}
            <div>
              <label for="mms-send-mode" class="mb-1 block text-xs font-medium text-gray-700 dark:text-gray-300">{$t('mms_send_mode_label')}</label>
              <select
                id="mms-send-mode"
                bind:value={profileSendMode}
                onchange={handleSendModeChange}
                disabled={profileSaving}
                class="w-full rounded-lg border border-gray-300 bg-white px-2.5 py-1.5 text-xs text-gray-900 focus:border-transparent focus:outline-none focus:ring-2 focus:ring-blue-500 dark:border-zinc-700 dark:bg-zinc-950 dark:text-gray-100"
              >
                <option value="modem_direct_attachment_upload">{$t('mms_send_mode_direct')}</option>
                <option value="host_staged_attachment_upload">{$t('mms_send_mode_host')}</option>
              </select>
              <div class="mt-1 text-[11px] text-gray-500 dark:text-gray-400">{$t('mms_send_mode_desc')}</div>
            </div>
            <div>
              <label for="mms-apn" class="mb-1 block text-xs font-medium text-gray-700 dark:text-gray-300">{$t('mms_apn_label')}</label>
              <input
                id="mms-apn"
                type="text"
                bind:value={profileApn}
                class="w-full rounded-lg border border-gray-300 bg-white px-2.5 py-1.5 text-xs text-gray-900
                       focus:border-transparent focus:outline-none focus:ring-2 focus:ring-blue-500
                       dark:border-zinc-700 dark:bg-zinc-950 dark:text-gray-100"
              />
            </div>
            <div>
              <label for="mms-ftp-apn" class="mb-1 block text-xs font-medium text-gray-700 dark:text-gray-300">{$t('mms_ftp_apn_label')}</label>
              <input
                id="mms-ftp-apn"
                type="text"
                bind:value={profileFtpApn}
                class="w-full rounded-lg border border-gray-300 bg-white px-2.5 py-1.5 text-xs text-gray-900
                       focus:border-transparent focus:outline-none focus:ring-2 focus:ring-blue-500
                       dark:border-zinc-700 dark:bg-zinc-950 dark:text-gray-100"
              />
            </div>
            <div>
              <label for="mms-mmsc" class="mb-1 block text-xs font-medium text-gray-700 dark:text-gray-300">{$t('mms_mmsc_label')}</label>
              <input
                id="mms-mmsc"
                type="text"
                bind:value={profileMmsc}
                class="w-full rounded-lg border border-gray-300 bg-white px-2.5 py-1.5 text-xs text-gray-900
                       focus:border-transparent focus:outline-none focus:ring-2 focus:ring-blue-500
                       dark:border-zinc-700 dark:bg-zinc-950 dark:text-gray-100"
              />
            </div>
            <div class="grid grid-cols-[1fr,110px] gap-2">
              <div>
                <label for="mms-proxy-host" class="mb-1 block text-xs font-medium text-gray-700 dark:text-gray-300">{$t('mms_proxy_host_label')}</label>
                <input
                  id="mms-proxy-host"
                  type="text"
                  bind:value={profileProxyHost}
                  class="w-full rounded-lg border border-gray-300 bg-white px-2.5 py-1.5 text-xs text-gray-900
                         focus:border-transparent focus:outline-none focus:ring-2 focus:ring-blue-500
                         dark:border-zinc-700 dark:bg-zinc-950 dark:text-gray-100"
                />
              </div>
              <div>
                <label for="mms-proxy-port" class="mb-1 block text-xs font-medium text-gray-700 dark:text-gray-300">{$t('mms_proxy_port_label')}</label>
                <input
                  id="mms-proxy-port"
                  type="text"
                  inputmode="numeric"
                  pattern="[0-9]*"
                  bind:value={profileProxyPort}
                  class="w-full rounded-lg border border-gray-300 bg-white px-2.5 py-1.5 text-xs text-gray-900
                         focus:border-transparent focus:outline-none focus:ring-2 focus:ring-blue-500
                         dark:border-zinc-700 dark:bg-zinc-950 dark:text-gray-100"
                />
              </div>
            </div>

            {#if profileError}
              <div class="flex items-start gap-2 rounded-lg bg-red-50 p-2.5 text-xs text-red-600 dark:bg-red-900/20 dark:text-red-400">
                <Icon icon="carbon:warning" class="mt-0.5 h-3.5 w-3.5 shrink-0" />
                <span>{profileError}</span>
              </div>
            {/if}
            {#if profileSuccess}
              <div class="flex items-start gap-2 rounded-lg bg-green-50 p-2.5 text-xs text-green-600 dark:bg-green-900/20 dark:text-green-400">
                <Icon icon="carbon:checkmark" class="mt-0.5 h-3.5 w-3.5 shrink-0" />
                <span>{profileSuccess}</span>
              </div>
            {/if}

            <button
              onclick={saveProfile}
              disabled={profileSaving || !selectedSimId}
              class="inline-flex w-full items-center justify-center gap-2 rounded-lg border border-gray-200 px-3 py-1.5
                     text-xs font-medium text-gray-700 transition-colors hover:bg-gray-50
                     disabled:cursor-not-allowed disabled:opacity-60
                     dark:border-zinc-700 dark:text-gray-300 dark:hover:bg-zinc-800"
            >
              {#if profileSaving}
                <Icon icon="carbon:loading" class="h-3.5 w-3.5 animate-spin" />
              {:else}
                <Icon icon="carbon:save" class="h-3.5 w-3.5" />
              {/if}
              {$t('btn_save_profile')}
            </button>
          {/if}
        </div>
      </section>
    </div>

    <!-- ── Right column: history ───────────────────────────────────────────── -->
    <section class="overflow-hidden rounded-xl border border-gray-200 bg-white shadow-sm dark:border-gray-700 dark:bg-gray-800">
      <div class="flex items-center justify-between border-b border-gray-200 px-4 py-3 dark:border-gray-700">
        <div>
          <div class="font-semibold">{$t('mms_history_title')}</div>
        </div>
        <div class="flex items-center gap-2 text-xs text-gray-500 dark:text-gray-400">
          <button
            onclick={() => loadHistory(historyPage - 1)}
            disabled={historyPage <= 1 || historyLoading}
            class="rounded-lg border border-gray-200 p-1.5 disabled:cursor-not-allowed disabled:opacity-40 dark:border-zinc-700"
            aria-label="Previous page"
          >
            <Icon icon="carbon:chevron-left" class="h-3.5 w-3.5" />
          </button>
          <span>{historyPage} / {totalPages} · {historyTotal} total</span>
          <button
            onclick={() => loadHistory(historyPage + 1)}
            disabled={historyPage >= totalPages || historyLoading}
            class="rounded-lg border border-gray-200 p-1.5 disabled:cursor-not-allowed disabled:opacity-40 dark:border-zinc-700"
            aria-label="Next page"
          >
            <Icon icon="carbon:chevron-right" class="h-3.5 w-3.5" />
          </button>
        </div>
      </div>

      {#if historyLoading && history.length === 0}
        <div class="flex h-64 items-center justify-center text-gray-500">
          <Icon icon="carbon:loading" class="mr-2 h-6 w-6 animate-spin" />
          Loading...
        </div>
      {:else if historyError}
        <div class="p-4 text-sm text-red-600 dark:text-red-400">{historyError}</div>
      {:else}
        <div class="overflow-x-auto">
          <table class="min-w-full text-sm">
            <thead class="bg-gray-50 dark:bg-gray-700/50">
              <tr>
                <th class="px-4 py-2 text-left font-medium">{$t('col_time')}</th>
                <th class="px-4 py-2 text-left font-medium">{$t('col_to')}</th>
                <th class="px-4 py-2 text-left font-medium">{$t('col_subject')}</th>
                <th class="px-4 py-2 text-left font-medium">{$t('col_status')}</th>
                <th class="px-4 py-2 text-left font-medium">{$t('col_error')}</th>
                <th class="px-4 py-2 text-right font-medium"></th>
              </tr>
            </thead>
            <tbody class="divide-y divide-gray-100 dark:divide-gray-700">
              {#each history as job (job.id)}
                <tr class="align-top hover:bg-gray-50 dark:hover:bg-gray-700/40">
                  <td class="whitespace-nowrap px-4 py-3 text-xs text-gray-500 dark:text-gray-400">
                    {formatDate(job.created_at)}
                  </td>
                  <td class="px-4 py-3">{job.to_number}</td>
                  <td class="max-w-[160px] truncate px-4 py-3" title={job.subject ?? ''}>{job.subject || '-'}</td>
                  <td class="px-4 py-3">
                    <span class={`inline-flex rounded-full px-2 py-0.5 text-xs font-medium ${statusBadgeClass(job.status)}`}>
                      {statusLabel(job.status)}
                    </span>
                  </td>
                  <td class="max-w-[220px] truncate px-4 py-3 text-xs text-gray-500 dark:text-gray-400" title={job.error_message ?? ''}>
                    {job.error_message || '-'}
                  </td>
                  <td class="px-4 py-3 text-right">
                    <button
                      onclick={() => openDetail(job)}
                      class="rounded-lg border border-gray-200 px-2 py-1 text-xs text-gray-600 hover:bg-gray-50
                             dark:border-zinc-700 dark:text-gray-300 dark:hover:bg-zinc-800"
                    >
                      {$t('btn_view')}
                    </button>
                  </td>
                </tr>
              {:else}
                <tr>
                  <td colspan="6" class="px-4 py-8 text-center text-gray-500 dark:text-gray-400">
                    {$t('mms_no_history')}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </section>
  </div>

  <!-- ── Inbox: detected MMS notifications + fetched/decoded content ── -->
  <div class="mx-auto max-w-7xl px-4 pb-6">
    <section class="overflow-hidden rounded-xl border border-gray-200 bg-white shadow-sm dark:border-gray-700 dark:bg-gray-800">
      <div class="flex items-center justify-between border-b border-gray-200 px-4 py-3 dark:border-gray-700">
        <div>
          <div class="font-semibold">{$t('mms_inbox_title')}</div>
          <div class="text-xs text-gray-500 dark:text-gray-400">{$t('mms_inbox_desc')}</div>
        </div>
        <div class="flex items-center gap-2 text-xs text-gray-500 dark:text-gray-400">
          <button
            onclick={() => loadInbox(inboxPage - 1)}
            disabled={inboxPage <= 1 || inboxLoading}
            class="rounded-lg border border-gray-200 p-1.5 disabled:cursor-not-allowed disabled:opacity-40 dark:border-zinc-700"
            aria-label="Previous page"
          >
            <Icon icon="carbon:chevron-left" class="h-3.5 w-3.5" />
          </button>
          <span>{inboxPage} / {inboxTotalPages} · {inboxTotal} total</span>
          <button
            onclick={() => loadInbox(inboxPage + 1)}
            disabled={inboxPage >= inboxTotalPages || inboxLoading}
            class="rounded-lg border border-gray-200 p-1.5 disabled:cursor-not-allowed disabled:opacity-40 dark:border-zinc-700"
            aria-label="Next page"
          >
            <Icon icon="carbon:chevron-right" class="h-3.5 w-3.5" />
          </button>
        </div>
      </div>

      {#if inboxLoading && inbox.length === 0}
        <div class="flex h-40 items-center justify-center text-gray-500">
          <Icon icon="carbon:loading" class="mr-2 h-6 w-6 animate-spin" />
          Loading...
        </div>
      {:else if inboxError}
        <div class="p-4 text-sm text-red-600 dark:text-red-400">{inboxError}</div>
      {:else}
        <div class="overflow-x-auto">
          <table class="min-w-full text-sm">
            <thead class="bg-gray-50 dark:bg-gray-700/50">
              <tr>
                <th class="px-4 py-2 text-left font-medium">{$t('col_time')}</th>
                <th class="px-4 py-2 text-left font-medium">{$t('col_sender')}</th>
                <th class="px-4 py-2 text-left font-medium">{$t('col_received_on')}</th>
                <th class="px-4 py-2 text-left font-medium">{$t('col_report_recipient')}</th>
                <th class="px-4 py-2 text-left font-medium">{$t('col_transaction_id')}</th>
                <th class="px-4 py-2 text-left font-medium">{$t('col_size')}</th>
                <th class="px-4 py-2 text-left font-medium">{$t('col_status')}</th>
                <th class="px-4 py-2 text-right font-medium"></th>
              </tr>
            </thead>
            <tbody class="divide-y divide-gray-100 dark:divide-gray-700">
              {#each inbox as item (item.id)}
                <tr class="align-top hover:bg-gray-50 dark:hover:bg-gray-700/40">
                  <td class="whitespace-nowrap px-4 py-3 text-xs text-gray-500 dark:text-gray-400">
                    {formatDate(item.created_at)}
                  </td>
                  <td class="px-4 py-3">{mmsSenderDisplay(item)}</td>
                  <td class="px-4 py-3">{mmsReceiverDisplay(item)}</td>
                  <td class="px-4 py-3">{normalizeMmsSender(item.report_recipient) || '-'}</td>
                  <td class="max-w-[220px] truncate px-4 py-3 font-mono text-xs" title={item.transaction_id}>
                    {item.transaction_id}
                  </td>
                  <td class="px-4 py-3">{formatMmsSize(item.message_size)}</td>
                  <td class="px-4 py-3">
                    <span class={`inline-flex rounded-full px-2 py-0.5 text-xs font-medium ${inboxStatusBadgeClass(item.status)}`}>
                      {inboxStatusLabel(item.status)}
                    </span>
                  </td>
                  <td class="px-4 py-3 text-right">
                    <button
                      onclick={() => openInboxDetail(item)}
                      class="rounded-lg border border-gray-200 px-2 py-1 text-xs text-gray-600 hover:bg-gray-50
                             dark:border-zinc-700 dark:text-gray-300 dark:hover:bg-zinc-800"
                    >
                      {$t('btn_view')}
                    </button>
                  </td>
                </tr>
              {:else}
                <tr>
                  <td colspan="8" class="px-4 py-8 text-center text-gray-500 dark:text-gray-400">
                    {$t('mms_inbox_no_data')}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </section>
  </div>
</div>

<Modal isOpen={inboxDetailOpen} onClose={closeInboxDetail} maxWidth="max-w-lg">
  <div class="flex items-center justify-between border-b border-gray-200 px-4 py-3 dark:border-zinc-700">
    <div class="font-semibold text-gray-800 dark:text-gray-100">{$t('mms_inbox_detail_title')}</div>
    <button onclick={closeInboxDetail} class="rounded-lg p-1.5 hover:bg-gray-100 dark:hover:bg-zinc-800" aria-label="Close">
      <Icon icon="carbon:close" class="h-4 w-4" />
    </button>
  </div>
  <div class="flex-1 overflow-auto p-4 text-sm">
    {#if inboxDetailItem}
      <dl class="space-y-2">
        <div class="flex justify-between gap-4">
          <dt class="text-gray-500 dark:text-gray-400">{$t('col_sender')}</dt>
          <dd class="font-medium">{mmsSenderDisplay(inboxDetailItem)}</dd>
        </div>
        {#if inboxDetailItem.subject}
          <div class="flex justify-between gap-4">
            <dt class="text-gray-500 dark:text-gray-400">{$t('mms_inbox_subject_label')}</dt>
            <dd class="break-all font-medium">{inboxDetailItem.subject}</dd>
          </div>
        {/if}
        {#if inboxDetailItem.from_address}
          <div class="flex justify-between gap-4">
            <dt class="text-gray-500 dark:text-gray-400">{$t('mms_inbox_from_label')}</dt>
            <dd class="break-all font-medium">{inboxDetailItem.from_address}</dd>
          </div>
        {/if}
        <div class="flex justify-between gap-4">
          <dt class="text-gray-500 dark:text-gray-400">{$t('col_transaction_id')}</dt>
          <dd class="break-all font-mono text-xs">{inboxDetailItem.transaction_id}</dd>
        </div>
        <div>
          <dt class="mb-1 text-gray-500 dark:text-gray-400">{$t('col_content_location')}</dt>
          <dd class="break-all rounded-lg bg-gray-50 p-2 font-mono text-xs dark:bg-zinc-900">
            {inboxDetailItem.content_location || '-'}
          </dd>
        </div>
        <div class="flex justify-between gap-4">
          <dt class="text-gray-500 dark:text-gray-400">{$t('col_size')}</dt>
          <dd class="font-medium">{formatMmsSize(inboxDetailItem.message_size)}</dd>
        </div>
        <div class="flex justify-between gap-4">
          <dt class="text-gray-500 dark:text-gray-400">{$t('col_expiry')}</dt>
          <dd class="font-medium">{formatDate(inboxDetailItem.expiry_at)}</dd>
        </div>
        <div class="flex justify-between gap-4">
          <dt class="text-gray-500 dark:text-gray-400">{$t('col_status')}</dt>
          <dd>
            <span class={`inline-flex rounded-full px-2 py-0.5 text-xs font-medium ${inboxStatusBadgeClass(inboxDetailItem.status)}`}>
              {inboxStatusLabel(inboxDetailItem.status)}
            </span>
          </dd>
        </div>
        {#if inboxDetailItem.error_message}
          <div>
            <dt class="mb-1 text-gray-500 dark:text-gray-400">{$t('col_error')}</dt>
            <dd class="break-words rounded-lg bg-red-50 p-2 text-xs text-red-600 dark:bg-red-900/20 dark:text-red-400">
              {inboxDetailItem.error_message}
            </dd>
          </div>
        {/if}
        {#if inboxDetailItem.status === 'fetched' || inboxDetailParts.length > 0}
          <div>
            <dt class="mb-1 text-gray-500 dark:text-gray-400">{$t('mms_inbox_parts_label')}</dt>
            <dd>
              {#if inboxDetailLoading}
                <span class="text-xs text-gray-400 dark:text-gray-500">…</span>
              {:else if inboxDetailParts.length > 0}
                <ul class="space-y-1">
                  {#each inboxDetailParts as part (part.id)}
                    <li class="flex items-center justify-between gap-2 rounded-lg bg-gray-50 px-2.5 py-1.5 text-xs dark:bg-zinc-900">
                      <span class="min-w-0 flex-1 truncate" title={part.content_type}>
                        {part.filename || part.content_type}
                      </span>
                      <span class="shrink-0 text-gray-400 dark:text-gray-500">{formatBytes(part.size_bytes)}</span>
                      <button
                        type="button"
                        class="shrink-0 font-medium text-blue-600 hover:underline dark:text-blue-400"
                        onclick={() => viewInboxPart(part)}
                      >
                        {$t('mms_inbox_view_part')}
                      </button>
                    </li>
                  {/each}
                </ul>
              {:else}
                <span class="text-xs text-gray-400 dark:text-gray-500">{$t('mms_inbox_no_parts')}</span>
              {/if}
            </dd>
          </div>
        {/if}
      </dl>
    {/if}
  </div>
</Modal>

<Modal isOpen={inboxPartPreviewOpen} onClose={() => (inboxPartPreviewOpen = false)} maxWidth="max-w-2xl">
  <div class="flex items-center justify-between border-b border-gray-200 px-4 py-3 dark:border-zinc-700">
    <div class="font-semibold text-gray-800 dark:text-gray-100">{inboxPartPreviewTitle || $t('mms_inbox_view_part')}</div>
    <button onclick={() => (inboxPartPreviewOpen = false)} class="rounded-lg p-1.5 hover:bg-gray-100 dark:hover:bg-zinc-800" aria-label="Close">
      <Icon icon="carbon:close" class="h-4 w-4" />
    </button>
  </div>
  <div class="max-h-[65vh] overflow-auto p-4 text-sm">
    <pre class="whitespace-pre-wrap break-words rounded-lg bg-gray-50 p-3 font-mono text-xs dark:bg-zinc-900">{inboxPartPreviewText || '-'}</pre>
  </div>
</Modal>

<Modal isOpen={detailOpen} onClose={closeDetail} maxWidth="max-w-lg">
  <div class="flex items-center justify-between border-b border-gray-200 px-4 py-3 dark:border-zinc-700">
    <div class="font-semibold text-gray-800 dark:text-gray-100">{$t('mms_detail_title')}</div>
    <button onclick={closeDetail} class="rounded-lg p-1.5 hover:bg-gray-100 dark:hover:bg-zinc-800" aria-label="Close">
      <Icon icon="carbon:close" class="h-4 w-4" />
    </button>
  </div>
  <div class="flex-1 overflow-auto p-4 text-sm">
    {#if detailJob}
      <dl class="space-y-2">
        <div class="flex justify-between gap-4">
          <dt class="text-gray-500 dark:text-gray-400">{$t('col_to')}</dt>
          <dd class="font-medium">{detailJob.to_number}</dd>
        </div>
        <div class="flex justify-between gap-4">
          <dt class="text-gray-500 dark:text-gray-400">{$t('col_subject')}</dt>
          <dd class="font-medium">{detailJob.subject || '-'}</dd>
        </div>
        <div class="flex justify-between gap-4">
          <dt class="text-gray-500 dark:text-gray-400">{$t('col_status')}</dt>
          <dd>
            <span class={`inline-flex rounded-full px-2 py-0.5 text-xs font-medium ${statusBadgeClass(detailJob.status)}`}>
              {statusLabel(detailJob.status)}
            </span>
          </dd>
        </div>
        <div class="flex justify-between gap-4">
          <dt class="text-gray-500 dark:text-gray-400">{$t('mms_err_code_label')}</dt>
          <dd class="font-mono">{detailJob.quectel_err_code ?? '-'}</dd>
        </div>
        <div class="flex justify-between gap-4">
          <dt class="text-gray-500 dark:text-gray-400">{$t('mms_http_code_label')}</dt>
          <dd class="font-mono">{detailJob.http_response_code ?? '-'}</dd>
        </div>
        {#if detailJob.error_message}
          <div>
            <dt class="mb-1 text-gray-500 dark:text-gray-400">{$t('col_error')}</dt>
            <dd class="break-words rounded-lg bg-red-50 p-2 text-xs text-red-600 dark:bg-red-900/20 dark:text-red-400">
              {detailJob.error_message}
            </dd>
          </div>
        {/if}
        <div>
          <dt class="mb-1 text-gray-500 dark:text-gray-400">{$t('mms_attachments_label')}</dt>
          <dd>
            {#if detailAttachments.length > 0}
              <ul class="space-y-1">
                {#each detailAttachments as att}
                  <li class="flex items-center justify-between rounded-lg bg-gray-50 px-2.5 py-1.5 text-xs dark:bg-zinc-900">
                    <span class="truncate pr-2">{att.filename}</span>
                    <span class="shrink-0 text-gray-400 dark:text-gray-500">{formatBytes(att.size_bytes)}</span>
                  </li>
                {/each}
              </ul>
            {:else}
              <span class="text-xs text-gray-400 dark:text-gray-500">{$t('mms_no_files')}</span>
            {/if}
          </dd>
        </div>
      </dl>
    {/if}
  </div>
</Modal>

<script>
  import { onMount, onDestroy } from 'svelte';
  import Icon from '@iconify/svelte';
  import { EventSourcePolyfill } from 'event-source-polyfill';
  import { apiClient } from '../js/api.js';
  import { t } from '../js/i18n.js';
  import { displayPort } from '../js/modem.js';
  import { user } from '../stores/auth.js';
  import MsisdnEditor from '../components/MsisdnEditor.svelte';

  let { onBack = () => {} } = $props();

  let ports = $state([]);
  let loading = $state(true);
  let error = $state('');
  let message = $state('');
  let busy = $state(false);
  let portStatus = $state({});

  // The port whose session panel is expanded / active.
  let activePort = $state(null);
  let chip = $state(null);
  let profiles = $state([]);
  let cardResult = $state(null);
  let operationId = $state(sessionStorage.getItem('remote_esim_request_id') ?? '');
  let operationStatus = $state('');
  let operationResult = $state(null);
  const activeTarget = $derived(ports.find(port => port.com_port === activePort));
  const activeRental = $derived($user?.rentals?.find(rental => rental.com_port === activePort));

  function allowed(com, permission) {
    return ports.find(port => port.com_port === com)?.permissions?.includes(permission) ?? false;
  }

  function batchAllowed(permission) {
    return selectedList.every(com => allowed(com, permission));
  }

  // Download form
  let dl = $state({ smdp: '', matching_id: '', confirmation_code: '', imei: '', activation_code: '', phone_number: '' });

  // ── Batch selection / filtering ──────────────────────────────────────────
  let selected = $state(new Set());
  let filterText = $state('');
  let filterCapable = $state(false);
  let filterHasSim = $state(false);

  // Batch dialog + job state
  let batchMode = $state(null); // null | 'download' | 'delete' | 'activate' | 'deactivate'
  let sources = $state([]); // ActivationCode[]
  let pairing = $state({}); // com_port -> index into `sources`
  let pasteText = $state('');
  let batchPhoneNumber = $state('');
  let batchOptions = $state({ auto_enable: false, replace_existing: false, stop_on_error: true });
  let job = $state(null); // JobSnapshot
  let batchBusy = $state(false);
  let sseConn = null;

  const sessionPort = $derived(ports.find((p) => p.session_active)?.com_port ?? null);

  const filteredPorts = $derived(
    ports.filter((p) => {
      if (filterCapable && !p.esim_capable) return false;
      if (filterHasSim && !p.sim_id) return false;
      const q = filterText.trim().toLowerCase();
      if (q) {
        const hay = `${displayPort(p.com_port)} ${p.slot_label ?? ''} ${p.mapping_label ?? ''} ${p.sim_id ?? ''}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    })
  );

  const selectedList = $derived(
    [...selected].sort((a, b) => (comNum(a) ?? 0) - (comNum(b) ?? 0))
  );
  const downloadReady = $derived(
    selectedList.length > 0 && selectedList.every((com) => {
      const idx = pairing[com];
      const code = idx != null ? sources[idx] : null;
      const phone = sourcePhone(code);
      return Boolean(code?.raw_lpa || code?.activation_code) && (!phone || normalizePhoneLabel(phone));
    })
  );
  const allFilteredSelected = $derived(
    filteredPorts.filter((p) => p.writable).length > 0 &&
      filteredPorts.filter((p) => p.writable).every((p) => selected.has(p.com_port))
  );

  function comNum(com) {
    const m = String(com).match(/(\d+)/);
    return m ? parseInt(m[1], 10) : null;
  }

  /** FetchApi rejects with {status, data} (not an Error), so pull the message out of the response body. */
  function describeError(e) {
    return e?.data?.error ?? e?.data?.message ?? e?.message ?? (e?.status ? `HTTP ${e.status}` : String(e));
  }

  function unwrapPayload(res) {
    const body = res?.data;
    if (body && typeof body === 'object' && 'data' in body) {
      return body.data;
    }
    return body;
  }

  async function loadPorts() {
    loading = true;
    error = '';
    try {
      const res = await apiClient.esimListPorts();
      const payload = unwrapPayload(res);
      ports = Array.isArray(payload) ? payload : [];
    } catch (e) {
      error = describeError(e);
    } finally {
      loading = false;
    }
  }

  function flash(msg) {
    message = msg;
    setTimeout(() => (message = ''), 4000);
  }

  function fail(e) {
    error = $t('esim_op_failed', { msg: describeError(e) });
    setTimeout(() => (error = ''), 6000);
  }

  function setPortStatus(com, status, op, msg = '') {
    portStatus = {
      ...portStatus,
      [com]: { status, op, msg },
    };
  }

  function statusBadge(status) {
    switch (status) {
      case 'ok':
        return 'bg-green-100 text-green-700';
      case 'failed':
        return 'bg-red-100 text-red-700';
      case 'running':
        return 'bg-amber-100 text-amber-700';
      case 'session':
        return 'bg-amber-100 text-amber-700';
      case 'skipped':
        return 'bg-slate-100 text-slate-500';
      default:
        return 'bg-slate-100 text-slate-500';
    }
  }

  function actionText(op) {
    if (!op) return '';
    const key = 'esim_status_action_' + op;
    const translated = $t(key);
    return translated === key ? op : translated;
  }

  function statusMetaForPort(p) {
    const batchItem = job?.items?.find((it) => it.com_port === p.com_port);
    if (batchItem) {
      return {
        status: batchItem.status,
        label: $t('esim_batch_item_' + batchItem.status),
        detail: batchItem.message ?? $t('esim_batch_op_' + job.op),
      };
    }

    const local = portStatus[p.com_port];
    if (local) {
      if (local.status === 'running') {
        return {
          status: 'running',
          label: $t('esim_status_running'),
          detail: actionText(local.op),
        };
      }
      if (local.status === 'ok') {
        return {
          status: 'ok',
          label: $t('esim_status_ok'),
          detail: actionText(local.op),
        };
      }
      return {
        status: 'failed',
        label: $t('esim_status_failed'),
        detail: local.msg ? `${actionText(local.op)}: ${local.msg}` : actionText(local.op),
      };
    }

    if (p.last_status) {
      const st = String(p.last_status).toLowerCase() === 'ok' ? 'ok' : 'failed';
      const detailParts = [];
      const opText = actionText(p.last_op);
      if (opText) detailParts.push(opText);
      if (p.last_message && String(p.last_message).toLowerCase() !== 'ok') {
        detailParts.push(p.last_message);
      }
      if (p.last_at) detailParts.push(p.last_at);
      return {
        status: st,
        label: st === 'ok' ? $t('esim_status_ok') : $t('esim_status_failed'),
        detail: detailParts.join(' | '),
      };
    }

    if (p.session_active || p.esim_mode) {
      return {
        status: 'session',
        label: $t('esim_status_session'),
        detail: $t('esim_mode_esim'),
      };
    }

    return {
      status: 'idle',
      label: $t('esim_status_idle'),
      detail: '',
    };
  }

  async function enter(com) {
    busy = true;
    error = '';
    setPortStatus(com, 'running', 'enter');
    try {
      await apiClient.esimEnter(com);
      cardResult = null;
      dl = { smdp: '', matching_id: '', confirmation_code: '', imei: '', activation_code: '', phone_number: '' };
      activePort = com;
      await loadPorts();
      await refreshSession(com);
      setPortStatus(com, 'ok', 'enter');
      flash($t('esim_op_ok'));
    } catch (e) {
      setPortStatus(com, 'failed', 'enter', describeError(e));
      fail(e);
    } finally {
      busy = false;
    }
  }

  async function exit(com) {
    busy = true;
    error = '';
    setPortStatus(com, 'running', 'exit');
    try {
      await apiClient.esimExit(com);
      if (activePort === com) {
        activePort = null;
        chip = null;
        profiles = [];
      }
      await loadPorts();
      setPortStatus(com, 'ok', 'exit');
      flash($t('esim_op_ok'));
    } catch (e) {
      setPortStatus(com, 'failed', 'exit', describeError(e));
      fail(e);
    } finally {
      busy = false;
    }
  }

  async function reset(com) {
    busy = true;
    setPortStatus(com, 'running', 'reset');
    try {
      await apiClient.esimReset(com);
      if (activePort === com) {
        activePort = null;
        chip = null;
        profiles = [];
      }
      await loadPorts();
      setPortStatus(com, 'ok', 'reset');
      flash($t('esim_op_ok'));
    } catch (e) {
      setPortStatus(com, 'failed', 'reset', describeError(e));
      fail(e);
    } finally {
      busy = false;
    }
  }

  async function refreshSession(com) {
    try {
      chip = unwrapPayload(await apiClient.esimChip(com)) ?? null;
      const p = unwrapPayload(await apiClient.esimProfiles(com)) ?? [];
      profiles = Array.isArray(p) ? p : [];
    } catch (e) {
      chip = null;
      profiles = [];
      fail(e);
    }
  }

  async function enableProfile(com, iccid) {
    busy = true;
    setPortStatus(com, 'running', 'enable');
    try {
      await apiClient.esimEnable(com, iccid, true);
      await refreshSession(com);
      // Keep ports out of long-held management mode: auto-exit after activation.
      await apiClient.esimExit(com);
      if (activePort === com) {
        activePort = null;
        chip = null;
        profiles = [];
      }
      await loadPorts();
      setPortStatus(com, 'ok', 'exit');
      flash($t('esim_op_ok'));
    } catch (e) {
      setPortStatus(com, 'failed', 'enable', describeError(e));
      fail(e);
    } finally {
      busy = false;
    }
  }

  async function disableProfile(com, iccid) {
    busy = true;
    setPortStatus(com, 'running', 'disable');
    try {
      await apiClient.esimDisable(com, iccid, true);
      await refreshSession(com);
      setPortStatus(com, 'ok', 'disable');
      flash($t('esim_op_ok'));
    } catch (e) {
      setPortStatus(com, 'failed', 'disable', describeError(e));
      fail(e);
    } finally {
      busy = false;
    }
  }

  async function deleteProfile(com, iccid) {
    if (!confirm($t('esim_confirm_delete'))) return;
    busy = true;
    setPortStatus(com, 'running', 'delete');
    try {
      await apiClient.esimDelete(com, iccid);
      await refreshSession(com);
      setPortStatus(com, 'ok', 'delete');
      flash($t('esim_op_ok'));
    } catch (e) {
      setPortStatus(com, 'failed', 'delete', describeError(e));
      fail(e);
    } finally {
      busy = false;
    }
  }

  async function nicknameProfile(com, iccid) {
    const name = prompt($t('esim_nickname_prompt'));
    if (name == null) return;
    busy = true;
    setPortStatus(com, 'running', 'nickname');
    try {
      await apiClient.esimNickname(com, iccid, name);
      await refreshSession(com);
      setPortStatus(com, 'ok', 'nickname');
      flash($t('esim_op_ok'));
    } catch (e) {
      setPortStatus(com, 'failed', 'nickname', describeError(e));
      fail(e);
    } finally {
      busy = false;
    }
  }

  async function download(com) {
    busy = true;
    setPortStatus(com, 'running', 'download');
    try {
      const body = {};
      for (const [k, v] of Object.entries(dl)) if (v) body[k] = v;
      if (!body.phone_number && body.activation_code) {
        const matched = sources.find((c) => c.raw_lpa === body.activation_code);
        const phone = normalizePhoneLabel(matched?.label ?? null);
        if (phone) body.phone_number = phone;
      }
      cardResult = unwrapPayload(await apiClient.esimDownload(com, body));
      await refreshSession(com);
      setPortStatus(com, 'ok', 'download');
      flash($t('esim_op_ok'));
    } catch (e) {
      setPortStatus(com, 'failed', 'download', describeError(e));
      fail(e);
    } finally {
      busy = false;
    }
  }

  function iccidOf(p) {
    return p.iccid ?? p.ICCID ?? '';
  }

  async function provision(com) {
    busy = true;
    error = '';
    cardResult = null;
    try {
      cardResult = unwrapPayload(await apiClient.esimProvision(com, dl));
      await refreshSession(com);
      if (cardResult?.partial) error = cardResult.warning ?? cardResult.notification_warning ?? 'Provisioning completed partially';
      else flash('Profile installation and readback completed');
    } catch (caught) {
      error = describeError(caught);
    } finally {
      busy = false;
    }
  }

  async function writeMsisdn(com, iccid) {
    busy = true;
    error = '';
    cardResult = null;
    try {
      cardResult = unwrapPayload(await apiClient.esimWriteMsisdn(com, iccid, dl.phone_number));
      flash('MSISDN verified on card');
    } catch (caught) {
      error = describeError(caught);
    } finally {
      busy = false;
    }
  }
  function providerOf(p) {
    return p.serviceProviderName ?? p.provider ?? '';
  }
  function nameOf(p) {
    return p.profileNickname ?? p.profileName ?? p.nickname ?? '';
  }
  function stateOf(p) {
    const s = (p.profileState ?? p.state ?? '').toString().toLowerCase();
    return s.includes('enable') ? 'enabled' : 'disabled';
  }

  // ── Selection ────────────────────────────────────────────────────────────
  function toggleSelect(com) {
    const s = new Set(selected);
    s.has(com) ? s.delete(com) : s.add(com);
    selected = s;
  }
  function toggleSelectAll() {
    const s = new Set(selected);
    const capable = filteredPorts.filter((p) => p.writable);
    if (allFilteredSelected) {
      capable.forEach((p) => s.delete(p.com_port));
    } else {
      capable.forEach((p) => s.add(p.com_port));
    }
    selected = s;
  }
  function clearSelection() {
    selected = new Set();
  }

  // ── Batch sources / pairing ──────────────────────────────────────────────
  async function loadSources() {
    batchBusy = true;
    try {
      const res = await apiClient.esimScanSources();
      sources = unwrapPayload(res) ?? [];
    } catch (e) {
      fail(e);
      sources = [];
    } finally {
      batchBusy = false;
    }
  }

  async function uploadSources(ev) {
    const files = ev.target?.files;
    if (!files || !files.length) return;
    batchBusy = true;
    try {
      const res = await apiClient.esimUploadSources(files);
      const codes = unwrapPayload(res) ?? [];
      sources = [...sources, ...codes];
      buildPairing();
    } catch (e) {
      fail(e);
    } finally {
      batchBusy = false;
      if (ev.target) ev.target.value = '';
    }
  }

  function addPasted() {
    const lines = pasteText.split(/\r?\n/);
    const added = [];
    for (const line of lines) {
      const m = line.match(/LPA:1\$([^$\s]+)\$([^$\s]+)(?:\$([^$\s]+))?/i);
      if (m) {
        added.push({
          raw_lpa: `LPA:1$${m[1]}$${m[2]}${m[3] ? '$' + m[3] : ''}`,
          smdp: m[1],
          matching_id: m[2],
          confirmation_code: m[3] || null,
          source: 'manual',
          port_hint: null,
          label: null,
        });
      }
    }
    sources = [...sources, ...added];
    pasteText = '';
    buildPairing();
  }

  /** Auto-pair codes to selected ports: filename hint first, then sequential. */
  function buildPairing() {
    const map = {};
    const used = new Set();
    for (const com of selectedList) {
      const n = comNum(com);
      const idx = sources.findIndex((c, i) => !used.has(i) && c.port_hint === n);
      if (idx >= 0) {
        map[com] = idx;
        used.add(idx);
      }
    }
    for (const com of selectedList) {
      if (map[com] != null) continue;
      const idx = sources.findIndex((c, i) => !used.has(i));
      if (idx >= 0) {
        map[com] = idx;
        used.add(idx);
      }
    }
    pairing = map;
  }
  function setPair(com, value) {
    pairing = { ...pairing, [com]: value === '' ? null : Number(value) };
  }
  function codeLabel(c) {
    if (!c) return '';
    return `${c.smdp} / ${c.matching_id}${c.label ? ' (' + c.label + ')' : ''}`;
  }

  function normalizePhoneLabel(label) {
    if (!label) return null;
    const phone = String(label).trim();
    return /^\+?[0-9]{1,20}$/.test(phone) ? phone : null;
  }

  function sourcePhone(code) {
    return code?.phone_number ?? normalizePhoneLabel(code?.label) ?? '';
  }

  function setSourcePhone(com, phone) {
    const index = pairing[com];
    if (index == null || !sources[index]) return;
    sources = sources.map((source, sourceIndex) => sourceIndex === index
      ? { ...source, phone_number: phone, phone_status: 'edited' }
      : source);
  }

  function phoneStatus(code) {
    if (!code || code.phone_status === 'edited') return '';
    if (code.phone_status === 'recognized') return $t('esim_msisdn_recognized');
    if (code.phone_status === 'unavailable') return $t('esim_msisdn_unavailable');
    return $t('esim_msisdn_not_found');
  }

  // ── Batch dialog + run ───────────────────────────────────────────────────
  async function openBatch(op) {
    if (sessionPort) {
      fail({ data: { error: $t('esim_batch_blocked_session', { port: displayPort(sessionPort) }) } });
      return;
    }
    if (selected.size === 0) return;
    batchMode = op;
    job = null;
    if (op === 'download') {
      await loadSources();
      buildPairing();
    }
  }
  function closeBatch() {
    batchMode = null;
    pasteText = '';
    batchPhoneNumber = '';
  }

  async function startDownload() {
    if (!downloadReady) return;
    const items = selectedList.map((com) => {
      const idx = pairing[com];
      const code = idx != null && sources[idx] ? sources[idx] : null;
      return {
        com_port: com,
        activation_code: code.raw_lpa || code.activation_code,
        phone_number: normalizePhoneLabel(sourcePhone(code)) ?? normalizePhoneLabel(batchPhoneNumber),
      };
    });
    closeBatch();
    await runBatch('download', items);
  }

  async function startSimpleBatch(op) {
    if (op === 'delete' && !confirm($t('esim_batch_confirm_delete', { n: selected.size }))) return;
    if (op === 'deactivate' && !confirm($t('esim_batch_confirm_deactivate', { n: selected.size }))) return;
    const items = selectedList.map((com) => ({ com_port: com }));
    await runBatch(op, items);
  }

  async function runBatch(op, items) {
    batchBusy = true;
    try {
      const payload = { op, items, ...batchOptions };
      await apiClient.esimStartBatch(payload);
      connectBatchSse();
      await pollBatchOnce();
      flash($t('esim_batch_started'));
    } catch (e) {
      fail(e);
    } finally {
      batchBusy = false;
    }
  }

  function authHeader() {
    return {};
  }

  function connectBatchSse() {
    if (sseConn) sseConn.close();
    sseConn = new EventSourcePolyfill('/api/esim/batch/events', {
      headers: authHeader(),
      heartbeatTimeout: 60000,
    });
    sseConn.addEventListener('batch', (e) => {
      try {
        const snap = JSON.parse(e.data);
        job = snap;
        if (snap && (snap.status === 'done' || snap.status === 'cancelled')) {
          if (sseConn) sseConn.close();
          sseConn = null;
          loadPorts();
        }
      } catch (_) {}
    });
    sseConn.onerror = () => pollBatchOnce();
  }

  async function pollBatchOnce() {
    try {
      const res = await apiClient.esimGetBatch();
      const snap = unwrapPayload(res);
      if (snap) {
        job = snap;
        if (snap.status === 'running' && !sseConn) connectBatchSse();
      }
    } catch (_) {}
  }

  async function cancelBatch() {
    try {
      await apiClient.esimCancelBatch();
    } catch (e) {
      fail(e);
    }
  }

  const jobRunning = $derived(job?.status === 'running');
  function itemBadge(status) {
    switch (status) {
      case 'ok':
        return 'bg-green-100 text-green-700';
      case 'failed':
        return 'bg-red-100 text-red-700';
      case 'running':
        return 'bg-amber-100 text-amber-700';
      case 'skipped':
        return 'bg-slate-100 text-slate-500';
      default:
        return 'bg-slate-100 text-slate-500';
    }
  }

  async function inspectOperation() {
    try {
      const result = unwrapPayload(await apiClient.esimOperationStatus(operationId));
      operationStatus = result.status;
      operationResult = result;
      if (result.result && result.target === activeTarget?.id) cardResult = result.result;
    } catch (e) {
      fail(e);
    }
  }

  onMount(() => {
    loadPorts();
    pollBatchOnce();
    const operationChanged = (event) => {
      operationId = event.detail;
      operationStatus = '';
      operationResult = null;
    };
    window.addEventListener('remote-esim-operation', operationChanged);
    return () => window.removeEventListener('remote-esim-operation', operationChanged);
  });
  onDestroy(() => {
    if (sseConn) sseConn.close();
  });
</script>

<div class="flex flex-col h-dvh w-screen bg-gray-50 dark:bg-zinc-950 text-sm font-sans">
  {#if operationId}
    <div class="flex flex-wrap items-center gap-2 px-4 py-2 border-b border-gray-200 dark:border-zinc-800">
      <span class="text-xs">Operation</span>
      <span class="font-mono text-xs break-all">{operationId}</span>
      <span class="text-xs">{operationStatus}</span>
      {#if operationResult}
        <span class="text-xs break-all">{operationResult.target} / {operationResult.action}</span>
        {#if operationResult.result?.iccid}
          <span class="text-xs font-mono break-all">ICCID: {operationResult.result.iccid}</span>
        {/if}
      {/if}
      <button onclick={inspectOperation} title="Inspect operation status" aria-label="Inspect operation status" class="p-2 hover:bg-gray-200 dark:hover:bg-zinc-800 rounded">
        <Icon icon="carbon:renew" class="w-4 h-4" />
      </button>
    </div>
  {/if}
  <header class="flex items-center justify-between gap-3 px-4 py-3 bg-white dark:bg-zinc-900 border-b border-gray-200 dark:border-zinc-800 shadow-sm shrink-0">
    <div class="flex items-center gap-3">
      <button
        onclick={onBack}
        class="inline-flex items-center gap-1.5 h-9 px-2.5 rounded-full bg-blue-600 text-white shadow-sm shadow-blue-600/30 hover:bg-blue-700 transition"
        aria-label="Back"
      >
        <Icon icon="carbon:arrow-left" class="w-4 h-4" />
        <span class="text-xs font-semibold">{$t('btn_back')}</span>
      </button>
      <div class="flex items-center gap-2">
        <Icon icon="carbon:sim-card" class="w-5 h-5 text-gray-500 dark:text-gray-400" />
        <h1 class="text-base font-semibold text-gray-800 dark:text-gray-100">{$t('esim_title')}</h1>
      </div>
    </div>
    <button
      class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-medium border border-gray-200 dark:border-zinc-700 text-gray-600 dark:text-gray-300 hover:bg-gray-50 dark:hover:bg-zinc-800 transition disabled:opacity-50"
      onclick={loadPorts}
      disabled={loading || busy}
    >
      <Icon icon="carbon:renew" class="w-4 h-4" />
      {$t('esim_refresh')}
    </button>
  </header>

  <div class="flex-1 overflow-auto p-4 sm:p-6">
    <p class="mb-3 text-sm text-gray-500 dark:text-gray-400">{$t('esim_subtitle')}</p>

    <!-- Lock banner -->
    <div class="mb-4 flex items-center justify-between gap-3 rounded-lg border px-4 py-2 text-sm {sessionPort ? 'border-amber-300 bg-amber-50 text-amber-800' : 'border-gray-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 text-gray-500 dark:text-gray-400'}">
      <span>{sessionPort ? $t('esim_lock_banner', { port: displayPort(sessionPort) }) : $t('esim_lock_free')}</span>
      {#if sessionPort}
        <button class="rounded bg-slate-700 px-2 py-1 text-xs text-white hover:bg-slate-600 disabled:opacity-50" onclick={() => exit(sessionPort)} disabled={busy || batchBusy}>
          {$t('esim_exit')}
        </button>
      {/if}
    </div>

    {#if message}
      <div class="mb-3 rounded-lg bg-green-50 dark:bg-green-900/20 px-4 py-2 text-sm text-green-700 dark:text-green-300">{message}</div>
    {/if}
    {#if error}
      <div class="mb-3 rounded-lg bg-red-50 dark:bg-red-900/20 px-4 py-2 text-sm text-red-700 dark:text-red-300">{error}</div>
    {/if}

    <!-- Filter + batch toolbar -->
    {#if !loading && ports.length > 0}
      <div class="mb-3 flex flex-wrap items-center gap-2 rounded-lg border border-slate-200 bg-white px-3 py-2">
        <div class="relative">
          <Icon icon="mdi:magnify" width="16" class="absolute left-2 top-2 text-slate-400" />
          <input
            class="w-48 rounded border border-slate-300 py-1 pl-7 pr-2 text-sm"
            placeholder={$t('esim_filter_placeholder')}
            bind:value={filterText}
          />
        </div>
        <label class="flex items-center gap-1 text-xs text-slate-600">
          <input type="checkbox" bind:checked={filterCapable} /> {$t('esim_filter_capable')}
        </label>
        <label class="flex items-center gap-1 text-xs text-slate-600">
          <input type="checkbox" bind:checked={filterHasSim} /> {$t('esim_filter_hassim')}
        </label>
        <span class="ml-auto text-xs text-slate-500">{$t('esim_selected_count', { n: selected.size })}</span>
      </div>

      {#if selected.size > 0}
        <div class="mb-3 flex flex-wrap items-center gap-2 rounded-lg border border-indigo-200 bg-indigo-50 px-3 py-2">
          <span class="text-sm font-medium text-indigo-800">{$t('esim_batch_bar', { n: selected.size })}</span>
          <button class="rounded bg-indigo-600 px-3 py-1 text-xs text-white hover:bg-indigo-500 disabled:opacity-50" onclick={() => openBatch('download')} disabled={busy || batchBusy || !!sessionPort || !batchAllowed('esim:download')}>{$t('esim_batch_download')}</button>
          <button class="rounded bg-green-600 px-3 py-1 text-xs text-white hover:bg-green-500 disabled:opacity-50" onclick={() => startSimpleBatch('activate')} disabled={busy || batchBusy || !!sessionPort || !batchAllowed('esim:enable')}>{$t('esim_batch_activate')}</button>
          <button class="rounded bg-slate-500 px-3 py-1 text-xs text-white hover:bg-slate-400 disabled:opacity-50" onclick={() => startSimpleBatch('deactivate')} disabled={busy || batchBusy || !!sessionPort || !batchAllowed('esim:disable')}>{$t('esim_batch_deactivate')}</button>
          <button class="rounded bg-red-500 px-3 py-1 text-xs text-white hover:bg-red-400 disabled:opacity-50" onclick={() => startSimpleBatch('delete')} disabled={busy || batchBusy || !!sessionPort || !batchAllowed('esim:delete')}>{$t('esim_batch_delete')}</button>
          <button class="ml-auto rounded bg-white px-3 py-1 text-xs text-slate-600 hover:bg-slate-100" onclick={clearSelection}>{$t('esim_batch_clear')}</button>
        </div>
      {/if}
    {/if}

    {#if loading}
      <p class="text-slate-500">…</p>
    {:else if ports.length === 0}
      <p class="rounded-lg bg-white p-6 text-center text-slate-500">{$t('esim_no_ports')}</p>
    {:else}
      <div class="overflow-x-auto border border-slate-200 bg-white">
        <table class="w-full text-sm">
          <thead class="bg-slate-100 text-left text-slate-600">
            <tr>
              <th class="px-3 py-2 w-8">
                <input type="checkbox" checked={allFilteredSelected} onchange={toggleSelectAll} aria-label="select all" />
              </th>
              <th class="px-4 py-2">{$t('esim_col_port')}</th>
              <th class="px-4 py-2 whitespace-nowrap">SIM Slot</th>
              <th class="px-4 py-2">Mapping</th>
              <th class="px-4 py-2">{$t('esim_col_capable')}</th>
              <th class="px-4 py-2">{$t('esim_col_mode')}</th>
              <th class="px-4 py-2">{$t('esim_col_status')}</th>
              <th class="px-4 py-2">{$t('esim_col_sim')}</th>
              <th class="px-4 py-2">{$t('esim_col_actions')}</th>
            </tr>
          </thead>
          <tbody>
            {#each filteredPorts as p (p.com_port)}
              {@const st = statusMetaForPort(p)}
              <tr class="border-t border-slate-100 {selected.has(p.com_port) ? 'bg-indigo-50/50' : ''}">
                <td class="px-3 py-2">
                  <input
                    type="checkbox"
                    checked={selected.has(p.com_port)}
                    disabled={!p.writable}
                    onchange={() => toggleSelect(p.com_port)}
                    aria-label={`select ${displayPort(p.com_port) || p.com_port}`}
                  />
                </td>
                <td class="px-4 py-2 font-medium text-slate-800">{displayPort(p.com_port) || '—'}</td>
                <td class="px-4 py-2 whitespace-nowrap">{p.slot_label}</td>
                <td class="px-4 py-2 whitespace-nowrap">
                  <div class="font-mono text-xs">{p.mapping_label}</div>
                  <div class="text-xs text-slate-500" title={p.error ?? ''}>{p.state}{p.protected ? ' / Protected' : ''}</div>
                </td>
                <td class="px-4 py-2">{p.esim_capable ? $t('esim_capable_yes') : $t('esim_capable_no')}</td>
                <td class="px-4 py-2">
                  <span class="rounded-full px-2 py-0.5 text-xs {p.esim_mode ? 'bg-amber-100 text-amber-700' : 'bg-slate-100 text-slate-600'}">
                    {p.esim_mode ? $t('esim_mode_esim') : $t('esim_mode_normal')}
                  </span>
                </td>
                <td class="px-4 py-2">
                  <span class="rounded-full px-2 py-0.5 text-xs {statusBadge(st.status)}">{st.label}</span>
                  {#if st.detail}
                    <div class="mt-0.5 max-w-[260px] truncate text-[11px] text-slate-500" title={st.detail}>{st.detail}</div>
                  {/if}
                </td>
                <td class="px-4 py-2 text-slate-500">{p.sim_id ?? '—'}</td>
                <td class="px-4 py-2">
                  <button class="mr-2 inline-flex items-center gap-1 rounded border px-2 py-1 text-xs disabled:opacity-50"
                    onclick={() => enter(p.com_port)} disabled={!p.esim_capable || busy || batchBusy || (!!sessionPort && sessionPort !== p.com_port)}>
                    <Icon icon="carbon:sim-card" class="h-4 w-4" /> Manage
                  </button>
                  <button
                    class="rounded bg-slate-500 px-2 py-1 text-xs text-white hover:bg-slate-400 disabled:opacity-50"
                    onclick={() => reset(p.com_port)}
                    disabled={!p.esim_capable || busy || batchBusy}
                  >
                    {$t('esim_status_action_reset')}
                  </button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>

    {/if}

    {#if activePort}
      <section class="mt-6 border-t border-slate-200 pt-4">
        <div class="mb-3 flex flex-wrap items-center justify-between gap-2">
          <h2 class="text-base font-semibold">{activeTarget?.slot_label ?? activePort}</h2>
          <button class="inline-flex items-center gap-1 rounded border px-2 py-1" onclick={() => refreshSession(activePort)} disabled={busy}>
            <Icon icon="carbon:renew" class="h-4 w-4" /> Refresh
          </button>
        </div>
        <dl class="mb-4 grid gap-1 text-xs sm:grid-cols-[5rem_1fr]">
          <dt class="text-slate-500">EID</dt><dd class="break-all font-mono">{chip?.eidValue ?? 'Unknown'}</dd>
          <dt class="text-slate-500">Mapping</dt><dd class="font-mono">{activeTarget?.mapping_label}</dd>
        </dl>
        <div class="overflow-x-auto">
          <table class="w-full text-left text-xs">
            <thead class="bg-slate-100"><tr><th class="p-2">ICCID</th><th class="p-2">Profile</th><th class="p-2">State</th><th class="p-2">Actions</th></tr></thead>
            <tbody>
              {#each profiles as profile (iccidOf(profile))}
                <tr class="border-b border-slate-100">
                  <td class="p-2 font-mono">{iccidOf(profile)}</td>
                  <td class="p-2">{nameOf(profile)}<div class="text-slate-500">{providerOf(profile)}</div></td>
                  <td class="p-2">{stateOf(profile)}</td>
                  <td class="flex flex-wrap gap-2 p-2">
                    <button class="rounded border px-2 py-1" onclick={() => enableProfile(activePort, iccidOf(profile))} disabled={busy || !activeTarget?.writable || !allowed(activePort, 'esim:enable') || stateOf(profile) === 'enabled'}>Enable</button>
                    <button class="rounded border px-2 py-1" onclick={() => disableProfile(activePort, iccidOf(profile))} disabled={busy || !activeTarget?.writable || !allowed(activePort, 'esim:disable') || stateOf(profile) !== 'enabled'}>Disable</button>
                    {#if $user?.is_admin}<button class="rounded border px-2 py-1" onclick={() => nicknameProfile(activePort, iccidOf(profile))} disabled={busy || !activeTarget?.writable || !allowed(activePort, 'esim:enable')}>Rename</button>{/if}
                    <button class="rounded border px-2 py-1 text-red-700" onclick={() => deleteProfile(activePort, iccidOf(profile))} disabled={busy || !activeTarget?.writable || !allowed(activePort, 'esim:delete') || stateOf(profile) === 'enabled'}>Delete</button>
                    <button class="rounded border px-2 py-1" title="Update EF.MSISDN on the SIM; issuer authentication may be required" onclick={() => writeMsisdn(activePort, iccidOf(profile))} disabled={busy || !activeTarget?.writable || !allowed(activePort, 'esim:msisdn') || !dl.phone_number || stateOf(profile) !== 'enabled'}>Write to SIM</button>
                  </td>
                </tr>
              {:else}
                <tr><td colspan="4" class="p-4 text-slate-500">No installed profiles</td></tr>
              {/each}
            </tbody>
          </table>
        </div>
        <form class="mt-4 grid gap-3 sm:grid-cols-2" onsubmit={(event) => { event.preventDefault(); provision(activePort); }}>
          <label class="grid gap-1 sm:col-span-2">Activation Code<input class="w-full rounded border px-2 py-1 font-mono text-xs" autocomplete="off" type="password" bind:value={dl.activation_code} /></label>
          {#if activeRental}
            <MsisdnEditor rental={activeRental} bind:value={dl.phone_number} />
          {:else}
            <label class="grid gap-1">MSISDN<input class="rounded border px-2 py-1" inputmode="tel" bind:value={dl.phone_number} /></label>
          {/if}
          <label class="grid gap-1">Confirmation Code<input class="rounded border px-2 py-1" type="password" autocomplete="off" bind:value={dl.confirmation_code} /></label>
          <div class="flex flex-wrap gap-2 sm:col-span-2">
            <button type="button" class="inline-flex items-center gap-1 rounded border px-3 py-1.5 disabled:opacity-50" onclick={() => download(activePort)} disabled={busy || !activeTarget?.writable || !allowed(activePort, 'esim:download') || !dl.activation_code}>
              <Icon icon="carbon:download" class="h-4 w-4" /> Download Only
            </button>
            <button type="submit" class="inline-flex items-center gap-1 rounded bg-green-700 px-3 py-1.5 text-white disabled:opacity-50" disabled={busy || !activeTarget?.writable || !allowed(activePort, 'esim:download') || !allowed(activePort, 'esim:enable') || (dl.phone_number && !allowed(activePort, 'esim:msisdn')) || !dl.activation_code}>
              <Icon icon="carbon:checkmark" class="h-4 w-4" /> Download and Enable
            </button>
          </div>
        </form>
        {#if cardResult}
          <div class="mt-3 text-xs">
            <span>ICCID: {cardResult.iccid ?? cardResult.identity?.iccid ?? 'Unknown'}</span>
            <span class="ml-3">MSISDN on card: {cardResult.msisdn_on_card ?? cardResult.msisdn?.msisdn_on_card ?? 'Not written'}</span>
            {#if cardResult.partial}
              <p class="mt-2 text-amber-800">Partial: {cardResult.warning ?? cardResult.notification_warning ?? 'Not all stages completed'}</p>
            {/if}
            {#if cardResult.stages}
              <dl class="mt-2 grid grid-cols-[minmax(0,10rem)_1fr] gap-1">
                {#each Object.entries(cardResult.stages) as [stage, status]}
                  <dt class="break-words">{stage}</dt><dd class="break-words">{status}</dd>
                {/each}
              </dl>
            {/if}
          </div>
        {/if}
      </section>
    {/if}

    <!-- Batch progress panel -->
    {#if job}
      <div class="mt-6 rounded-lg border border-slate-200 bg-white p-4">
        <div class="mb-3 flex items-center justify-between">
          <h2 class="font-semibold text-slate-800">
            {$t('esim_batch_progress')} —
            <span class="text-sm font-normal text-slate-500">{$t('esim_batch_op_' + job.op)}</span>
            <span class="ml-2 rounded-full px-2 py-0.5 text-xs {job.status === 'running' ? 'bg-amber-100 text-amber-700' : job.status === 'cancelled' ? 'bg-slate-100 text-slate-600' : 'bg-green-100 text-green-700'}">
              {$t('esim_batch_status_' + job.status)}
            </span>
          </h2>
          <div class="flex gap-2">
            {#if jobRunning}
              <button class="rounded bg-red-500 px-3 py-1 text-xs text-white hover:bg-red-400" onclick={cancelBatch}>{$t('esim_batch_cancel')}</button>
            {:else}
              <button class="rounded bg-slate-100 px-3 py-1 text-xs hover:bg-slate-200" onclick={() => (job = null)}>{$t('esim_batch_close')}</button>
            {/if}
          </div>
        </div>
        <table class="w-full text-sm">
          <thead class="bg-slate-100 text-left text-slate-600">
            <tr>
              <th class="px-3 py-2">{$t('esim_col_port')}</th>
              <th class="px-3 py-2">{$t('esim_col_state')}</th>
              <th class="px-3 py-2">{$t('esim_col_iccid')}</th>
              <th class="px-3 py-2">{$t('esim_batch_message')}</th>
            </tr>
          </thead>
          <tbody>
            {#each job.items as it (it.com_port)}
              <tr class="border-t border-slate-100">
                <td class="px-3 py-2 font-medium text-slate-800">{displayPort(it.com_port) || '—'}</td>
                <td class="px-3 py-2">
                  <span class="rounded-full px-2 py-0.5 text-xs {itemBadge(it.status)}">{$t('esim_batch_item_' + it.status)}</span>
                </td>
                <td class="px-3 py-2 font-mono text-xs">{it.iccid ?? '—'}</td>
                <td class="px-3 py-2 text-xs text-slate-500">{it.message ?? ''}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </div>
</div>

<!-- Batch download dialog -->
{#if batchMode === 'download'}
  <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4">
    <div class="max-h-[90vh] w-full max-w-3xl overflow-y-auto rounded-lg bg-white p-5 shadow-xl">
      <div class="mb-3 flex items-center justify-between">
        <h2 class="text-lg font-semibold text-slate-800">{$t('esim_batch_download_title')}</h2>
        <button class="rounded p-1 hover:bg-slate-100" onclick={closeBatch} aria-label="close">
          <Icon icon="mdi:close" width="20" />
        </button>
      </div>

      <!-- Source ingestion -->
      <div class="mb-4 rounded border border-slate-200 p-3">
        <div class="mb-2 flex flex-wrap items-center gap-2">
          <button class="rounded bg-slate-700 px-3 py-1 text-xs text-white hover:bg-slate-600 disabled:opacity-50" onclick={loadSources} disabled={batchBusy}>{$t('esim_src_scan')}</button>
          <label class="cursor-pointer rounded bg-slate-100 px-3 py-1 text-xs hover:bg-slate-200">
            {$t('esim_src_upload')}
            <input type="file" class="hidden" multiple accept="image/*,.txt,.csv" onchange={uploadSources} />
          </label>
          <span class="text-xs text-slate-500">{$t('esim_src_count', { n: sources.length })}</span>
        </div>
        <div class="mb-2">
          <input
            class="w-full rounded border border-slate-300 px-2 py-1 text-xs"
            type="tel"
            aria-label={$t('esim_msisdn_fallback')}
            placeholder={$t('esim_msisdn_fallback')}
            bind:value={batchPhoneNumber}
          />
        </div>
        <div class="flex gap-2">
          <textarea class="h-16 flex-1 rounded border border-slate-300 px-2 py-1 text-xs font-mono" placeholder={$t('esim_src_paste_placeholder')} bind:value={pasteText}></textarea>
          <button class="self-start rounded bg-slate-100 px-3 py-1 text-xs hover:bg-slate-200" onclick={addPasted}>{$t('esim_src_add')}</button>
        </div>
      </div>

      <!-- Options -->
      <div class="mb-3 flex flex-wrap gap-4 text-xs text-slate-600">
        <label class="flex items-center gap-1"><input type="checkbox" bind:checked={batchOptions.auto_enable} /> {$t('esim_opt_auto_enable')}</label>
        <label class="flex items-center gap-1"><input type="checkbox" bind:checked={batchOptions.stop_on_error} /> {$t('esim_opt_stop_on_error')}</label>
      </div>

      <!-- Pairing table -->
      <div class="overflow-x-auto">
      <table class="w-full min-w-[36rem] table-fixed text-sm">
        <thead class="bg-slate-100 text-left text-slate-600">
          <tr>
            <th class="w-24 px-3 py-2">{$t('esim_col_port')}</th>
            <th class="px-3 py-2">{$t('esim_batch_assigned_code')}</th>
            <th class="w-44 px-3 py-2">MSISDN</th>
          </tr>
        </thead>
        <tbody>
          {#each selectedList as com (com)}
            {@const source = pairing[com] != null ? sources[pairing[com]] : null}
            <tr class="border-t border-slate-100">
              <td class="px-3 py-2 font-medium text-slate-800">{displayPort(com)}</td>
              <td class="px-3 py-2">
                <select class="w-full rounded border border-slate-300 px-2 py-1 text-xs" value={pairing[com] ?? ''} onchange={(e) => setPair(com, e.target.value)}>
                  <option value="">{$t('esim_batch_no_code')}</option>
                  {#each sources as c, i (i)}
                    <option value={i}>{codeLabel(c)} — {c.source}</option>
                  {/each}
                </select>
              </td>
              <td class="px-3 py-2 align-top">
                <input
                  class="w-full rounded border border-slate-300 px-2 py-1 text-xs"
                  type="tel"
                  aria-label={`${displayPort(com)} MSISDN`}
                  value={sourcePhone(source)}
                  oninput={(event) => setSourcePhone(com, event.currentTarget.value)}
                  disabled={!source}
                  aria-invalid={Boolean(sourcePhone(source) && !normalizePhoneLabel(sourcePhone(source)))}
                />
                {#if sourcePhone(source) && !normalizePhoneLabel(sourcePhone(source))}
                  <p class="mt-1 text-xs text-red-600">{$t('esim_msisdn_invalid')}</p>
                {:else if phoneStatus(source)}
                  <p class="mt-1 text-xs text-slate-500">{phoneStatus(source)}</p>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
      </div>

      <div class="mt-4 flex justify-end gap-2">
        <button class="rounded bg-slate-100 px-4 py-2 text-sm hover:bg-slate-200" onclick={closeBatch}>{$t('esim_batch_cancel')}</button>
        <button class="rounded bg-indigo-600 px-4 py-2 text-sm text-white hover:bg-indigo-500 disabled:opacity-50" onclick={startDownload} disabled={batchBusy || !downloadReady}>{$t('esim_batch_start_download')}</button>
      </div>
    </div>
  </div>
{/if}

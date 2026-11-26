/** Show asterisk1 as COM1. Internal ids stay unchanged. */
export function displayPort(name) {
    if (name == null || String(name).trim() === '') return '';
    const raw = String(name).trim();
    if (/^asterisk$/i.test(raw)) return 'COM1';
    const match = raw.match(/^asterisk(\d+)\b/i);
    if (match) return `COM${match[1]}`;
    return raw;
}

/** Join rental/worker/modem labels such as asterisk and asterisk1. */
export function normalizePort(name) {
    const raw = String(name ?? '').trim().toLowerCase();
    if (!raw) return '';
    if (raw === 'asterisk' || raw === 'com1') return 'asterisk1';
    const asterisk = raw.match(/^asterisk(\d+)$/);
    if (asterisk) return `asterisk${asterisk[1]}`;
    const com = raw.match(/^com(\d+)$/);
    if (com) return com[1] === '1' ? 'asterisk1' : `asterisk${com[1]}`;
    return raw;
}

export function getModuleLabel(model) {
    if (!model) return '—';
    const normalized = String(model).trim().toUpperCase();
    if (normalized === 'EC20F' || normalized === 'EC25' || normalized === 'A7630C-LANS' || normalized === 'A7670C-LANS') {
        return '4G';
    }
    return model;
}

const networkStatuses = {
    0: { key: 'net_not_registered', cls: 'text-gray-400' },
    1: { key: 'net_home', cls: 'text-green-500' },
    2: { key: 'net_searching', cls: 'text-yellow-400' },
    3: { key: 'net_denied', cls: 'text-red-500' },
    4: { key: 'net_unknown', cls: 'text-gray-400' },
    5: { key: 'net_roaming', cls: 'text-blue-400' },
};

export function getNetworkStatus(registration) {
    const status = String(registration?.status ?? '').trim();
    if (!status) return networkStatuses[4];
    return networkStatuses[status] ?? {
        key: 'net_code', codeN: status, cls: 'text-gray-400',
    };
}
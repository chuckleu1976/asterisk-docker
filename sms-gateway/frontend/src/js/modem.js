/** Show asterisk1 as COM1. Internal ids stay unchanged. */
export function displayPort(name) {
    if (name == null || String(name).trim() === '') return '';
    const raw = String(name).trim();
    const match = raw.match(/^asterisk(\d+)\b/i);
    if (match) return `COM${match[1]}`;
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
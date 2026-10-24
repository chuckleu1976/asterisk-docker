export function tokenRequest(name, permissions) {
    return { name, permissions };
}

export function getTokenStatus(token, rentals, currentTime = Date.now()) {
    if (!token.active) return 'tokens_revoked';
    if (token.expires_at && Date.parse(token.expires_at) <= currentTime) return 'tokens_expired';
    const hasAccess = rentals.some(rental =>
        rental.active !== false && rental.valid !== false &&
        Date.parse(rental.starts_at) <= currentTime && currentTime < Date.parse(rental.expires_at) &&
        rental.permissions.some(permission => token.permissions.includes(permission))
    );
    return hasAccess ? 'tokens_active' : 'tokens_no_access';
}

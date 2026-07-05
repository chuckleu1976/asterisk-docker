use crate::db::Sms;

#[test]
fn inbox_uses_sender_id_expression() {
    assert_eq!(
        Sms::contact_name_expr_for_direction(false),
        "s.contact_id",
        "inbox must use raw sender id and not contact label"
    );
}

#[test]
fn sent_uses_contact_name_expression() {
    assert_eq!(
        Sms::contact_name_expr_for_direction(true),
        "COALESCE(c.name, s.contact_id)",
        "sent should still resolve contact name"
    );
}

#[test]
fn extract_phone_like_token_finds_number_in_message() {
    let found = Sms::extract_phone_like_token("hello from 37045 messages");
    assert_eq!(found.as_deref(), Some("37045"));
}

#[test]
fn extract_phone_like_token_returns_none_without_number() {
    let found = Sms::extract_phone_like_token("api smoke test from pos system");
    assert_eq!(found, None);
}

-- Preserve structured metadata from MMS delivery reports without overloading
-- notification sender/content fields.
ALTER TABLE mms_inbox ADD COLUMN report_recipient TEXT;
ALTER TABLE mms_inbox ADD COLUMN report_status INTEGER;

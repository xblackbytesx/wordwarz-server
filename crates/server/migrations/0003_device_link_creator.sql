-- Who made a device link: the person themselves, or an admin restoring an
-- account onto a new phone (People, New phone). Kept for the record.
ALTER TABLE device_links ADD COLUMN created_by uuid REFERENCES users (id) ON DELETE SET NULL;

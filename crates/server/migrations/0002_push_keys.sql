-- WebPush keys per endpoint (RFC 8291): the server encrypts every push to
-- them, so the push server relays ciphertext. NULL: an old connector without
-- keys; then the plaintext body is only an event and a game id.
ALTER TABLE push_endpoints ADD COLUMN pub_key text;
ALTER TABLE push_endpoints ADD COLUMN auth text;

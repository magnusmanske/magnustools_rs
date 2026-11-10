-- Dashboard service tokens are stored as lower-case SHA-256 hex. A case-
-- sensitive, single-byte collation matches the value exactly and keeps the
-- unique index small.
ALTER TABLE `dash_service`
  MODIFY `token_hash` char(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL;

Decode `_ModuleTable_` with the same URL-safe base64 path as chunked
module table exports before hashing, so equivalent exports record the
same `module_table_sha256` and malformed singleton values are rejected.

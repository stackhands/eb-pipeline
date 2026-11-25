Route remaining user-visible state writes through `util::write_atomic` and
add clippy disallowed-method guards so new direct `std::fs::write` or
`std::fs::File::create` output writers are rejected in library and binary
code.

Standard output is flushed before a write is checked, so output without a
trailing newline, such as the help and version text and the `from-commit` and
`gap` output, that the consumer cannot take is reported as
`cannot write stdout: ...` instead of being lost at exit.

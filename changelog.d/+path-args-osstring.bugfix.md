Companion commands now read path arguments as byte-preserving paths via
`args_os`, matching `eb-pipeline` behavior for non-UTF-8 file names instead of
panicking in argument parsing.

Route companion stdout writes through shared util helpers so all shipped
binaries report the same `cannot write stdout: ...` diagnostic and
expected exit code when stdout writes fail.

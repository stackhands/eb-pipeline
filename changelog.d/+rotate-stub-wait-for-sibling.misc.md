Make the rotate-overlap test's keep-logs stub wait only until the sibling target
enters rotation or finishes, instead of for an overlap that a passing run never
shows, and stop sharing a temporary file between the two stubs.

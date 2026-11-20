The summary-lock signal regression now writes its `SKIP:` notice directly
to a locked stderr handle, and only after a sustained unobservable
`/proc` wait-state window.

Add `--refuse-systemic-failure` to `eb-retry-list`. The flag classifies
failed logs and refuses output only when every failed line is one shared
infrastructure class (command path, disk full, fetch network, or missing
module tree). Ordinary build failures still produce retry lists.

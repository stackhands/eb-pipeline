The `watch` rebuild test reaps its `eb-pipeline watch` child before any assert runs. Its stub `eb` also stops waiting once `STUB_DIR` is gone, so a failing test leaves no process behind.

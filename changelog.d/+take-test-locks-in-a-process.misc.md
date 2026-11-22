Take lock files in tests from a separate process, so that a concurrent fork cannot keep a released lock alive.

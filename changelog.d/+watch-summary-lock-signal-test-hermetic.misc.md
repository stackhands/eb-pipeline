The `watch` summary lock signal test stops and reaps its `eb-pipeline watch` child before it fails, and clears `EB_PIPELINE_RUN_ID` so an inherited value cannot change the run.

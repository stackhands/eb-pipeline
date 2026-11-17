`eb-pipeline run --oom-preflight --node NODE` asks `eb-stack stack oom` about the planned packages and skips the build when its `oom` count is greater than zero.

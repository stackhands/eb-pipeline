`run` leaves out an `installed_modules_dirs` tree that does not exist yet and logs a WARN. On a new site prefix, `stack shard` refused the missing `modules/all` and the run exited 2 before any build.

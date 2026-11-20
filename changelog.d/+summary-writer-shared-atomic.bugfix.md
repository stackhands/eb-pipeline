Summary writes use the shared atomic writer, which syncs the file before
the rename. A symbolic link at the summary output path is now refused with
an error instead of being replaced by a regular file.

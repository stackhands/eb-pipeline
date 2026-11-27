`eb-junit` now gives every case a unique name. A case whose name lost
a character during escaping could share a name with another case,
and GitLab then showed only one of them.

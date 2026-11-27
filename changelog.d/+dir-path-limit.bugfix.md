The archive check counted a directory slash toward the 4096-byte path limit. Tar stores that slash on a directory member. The check now measures the path after removing that one slash.

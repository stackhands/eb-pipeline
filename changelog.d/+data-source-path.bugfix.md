EasyBuild downloads a data file into `EASYBUILD_SOURCEPATH_DATA` and reuses the first copy it finds.
EasyBuild fills sourcepath-data from sourcepath only while nothing else sets it.
A configuration file or the inherited environment could still give every target one shared data directory.
The run now exports `EASYBUILD_SOURCEPATH_DATA` from the same `filesystems.source_paths` entries, and that value overrides both.

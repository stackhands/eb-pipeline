cat > site.toml <<EOF
schema_version = 1
kind = "site"

[site]
name = "example"

[scheduler]
kind = "local"

[[targets]]
name = "cpu"

[filesystems]
scratch = "$WORK/scratch"
tmp = "$WORK/scratch/tmp"
build_path = "$WORK/scratch/build"
install_root = "$WORK/software"
log_root = "$WORK/logs"

[modules]
init = []
EOF

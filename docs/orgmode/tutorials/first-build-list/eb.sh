mkdir -p bin
cat > bin/eb <<'EOF'
#!/bin/sh
echo "eb $*"
env | grep '^EASYBUILD_' | sort
case "$1" in
  Broken-*) echo "ERROR: cannot build $1"; exit 1 ;;
esac
EOF
chmod +x bin/eb

# What the hiding service would do with the static rule, in shell, so that
# it can be run under any user and any sandbox:
#
#   hide.sh hide|restore <marker> <bus> <driver> <device>
#       creates or deletes the marker, then unbinds and binds the device
#   hide.sh open <node>...
#       opens each node read-write and says whether it could
#   hide.sh probe <file>...
#       writes a device name that doesn't exist to each sysfs file: "No such
#       device" means the write got through to the kernel
#
# Each step reports its own outcome; waiting for udev is the caller's.
set -u
step() {
  if output=$("$@" 2>&1); then
    echo "$label: ok"
  else
    echo "$label: failed: ${output##*: }"
  fi
}
uid=$(id -u)
echo "as uid=$uid $(grep -E '^Cap(Eff|Bnd)' /proc/self/status | tr -s '\t\n' '  ')"
case $1 in
  hide | restore)
    marker=/run/ctrlassist/hide/$2
    driver=/sys/bus/$3/drivers/$4
    if [ "$1" = hide ]; then
      label="create $marker" step touch "$marker"
    else
      label="delete $marker" step rm -f "$marker"
    fi
    label="unbind $5" step sh -c "echo $5 > $driver/unbind"
    label="bind $5" step sh -c "echo $5 > $driver/bind"
    ;;
  open)
    shift
    for node in "$@"; do
      label="open $node" step sh -c "true 3<>$node"
    done
    ;;
  probe)
    shift
    for file in "$@"; do
      label="write $file" step sh -c "echo nosuchdevice > $file"
    done
    ;;
esac

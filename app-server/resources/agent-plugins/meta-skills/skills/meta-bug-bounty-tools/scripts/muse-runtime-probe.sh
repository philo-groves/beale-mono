#!/bin/sh
# Run only in the researcher's own authorized Muse runtime cell.
set -eu

run_id=${1-}
case "$run_id" in
  ''|*[!a-f0-9]*) echo 'Expected a hexadecimal run ID.' >&2; exit 2 ;;
esac

printf 'BEALE_MUSE_BOUNDARY_V1\t%s\n' "$run_id"
printf 'uid\t%s\n' "$(id -u)"
if [ -r /proc/self/uid_map ]; then
  read -r inside_uid outside_uid mapped_count < /proc/self/uid_map
  printf 'uid_map\t%s %s %s\n' "$inside_uid" "$outside_uid" "$mapped_count"
else
  printf 'uid_map\tunavailable\n'
fi
if [ -r /proc/self/status ]; then
  cap_eff=$(awk '$1 == "CapEff:" { print $2 }' /proc/self/status)
  cap_bnd=$(awk '$1 == "CapBnd:" { print $2 }' /proc/self/status)
  printf 'cap_eff\t%s\n' "${cap_eff:-unavailable}"
  printf 'cap_bnd\t%s\n' "${cap_bnd:-unavailable}"
else
  printf 'cap_eff\tunavailable\n'
  printf 'cap_bnd\tunavailable\n'
fi
printf 'END_BEALE_MUSE_BOUNDARY_V1\t%s\n' "$run_id"

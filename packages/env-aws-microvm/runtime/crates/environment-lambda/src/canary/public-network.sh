set -euo pipefail

denied=(__DENIED__)
controls=(__CONTROLS__)
http_surfaces=(__HTTP_SURFACES__)
customer_environment_hosts=(__CUSTOMER_ENVIRONMENT_HOSTS__)

probe() {
  timeout 3 bash -c "exec 3<>/dev/tcp/$1/$2" >/dev/null 2>&1
}

probe_hosts() {
  local output=$1 ports=$2
  shift 2
  for host in "$@"; do
    (
      for port in $ports; do
        if probe "$host" "$port"; then
          printf '%s\n' "$host" >>"$output"
          break
        fi
      done
    ) &
  done
  wait
}

reachable_special=$(mktemp)
reachable_controls=$(mktemp)
trap 'rm -f "$reachable_special" "$reachable_controls"' EXIT
probe_hosts "$reachable_special" '80 443' "${denied[@]}"
if [[ -s $reachable_special ]]; then
  echo "special-use destinations accepted TCP: $(paste -sd, "$reachable_special")" >&2
  exit 1
fi
probe_hosts "$reachable_controls" '53 80 443' "${controls[@]}"
if [[ ! -s $reachable_controls ]]; then
  echo 'no public control was reachable' >&2
  exit 1
fi

observed_public_source=$(curl --silent --show-error --connect-timeout 3 --max-time 3 https://checkip.amazonaws.com/ 2>/dev/null | tr -d '[:space:]' || true)
if [[ ! $observed_public_source =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]]; then
  observed_public_source=unavailable
fi

for surface in "${http_surfaces[@]}"; do
  host=${surface%%|*}
  path=${surface#*|}
  status=$(curl --silent --show-error --output /dev/null --write-out '%{http_code}' --connect-timeout 3 --max-time 3 "https://$host$path" 2>/dev/null || true)
  if [[ $status != 403 ]]; then
    echo "Aex HTTPS surface did not return the expected source denial: $host status=${status:-unreachable} source=$observed_public_source" >&2
    exit 1
  fi
done

for host in "${customer_environment_hosts[@]}"; do
  websocket_status=$(curl --silent --show-error --output /dev/null --write-out '%{http_code}' --connect-timeout 3 --max-time 3 \
    --header 'Connection: Upgrade' --header 'Upgrade: websocket' \
    --header 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' --header 'Sec-WebSocket-Version: 13' \
    "https://$host/v1" 2>/dev/null || true)
  if [[ $websocket_status != 401 && $websocket_status != 403 ]]; then
    echo "customer Environment WebSocket did not return an authentication denial: $host" >&2
    exit 1
  fi
  management_status=$(curl --silent --show-error --output /dev/null --write-out '%{http_code}' --connect-timeout 3 --max-time 3 \
    --request POST --header 'Content-Length: 0' \
    "https://$host/v1/@connections/L0SM9cOFvHcCIhw%3D" 2>/dev/null || true)
  if [[ $management_status != 401 && $management_status != 403 ]]; then
    echo "customer Environment Management API did not return an authentication denial: $host" >&2
    exit 1
  fi
done

printf 'network_canary=ok denied=%s controls=%s surfaces=%s source=%s\n' \
  "${#denied[@]}" "${#controls[@]}" "$(( ${#http_surfaces[@]} + ${#customer_environment_hosts[@]} ))" "$observed_public_source"

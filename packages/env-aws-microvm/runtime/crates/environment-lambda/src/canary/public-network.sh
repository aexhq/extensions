set -euo pipefail
trap 'status=$?; (( status == 0 )) || printf "public network canary shell failed: status=%s\n" "$status" >&2' EXIT

denied=(__DENIED__)
controls=(__CONTROLS__)
http_surfaces=(__HTTP_SURFACES__)
customer_environment_hosts=(__CUSTOMER_ENVIRONMENT_HOSTS__)

probe_hosts() {
  local ports=$1 host port output url remote authority
  local -a urls=()
  shift
  for host in "$@"; do
    for port in $ports; do urls+=("telnet://$host:$port"); done
  done
  output=$(curl --disable --ipv4 --noproxy '*' --silent --output /dev/null \
    --parallel --parallel-immediate --parallel-max 32 \
    --connect-timeout 1.5 --max-time 1.5 \
    --write-out $'%{url_effective}\t%{remote_ip}\n' "${urls[@]}" 2>/dev/null || true)
  while IFS=$'\t' read -r url remote; do
    if [[ -n $remote ]]; then
      authority=${url#telnet://}
      printf '%s\n' "${authority%:*}"
    fi
  done <<<"$output" | sort -u
}

reachable_special=$(probe_hosts '80 443' "${denied[@]}")
if [[ -n $reachable_special ]]; then
  echo "special-use destinations accepted TCP: ${reachable_special//$'\n'/,}" >&2
  exit 1
fi
reachable_controls=$(probe_hosts '53 80 443' "${controls[@]}")
if [[ -z $reachable_controls ]]; then
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

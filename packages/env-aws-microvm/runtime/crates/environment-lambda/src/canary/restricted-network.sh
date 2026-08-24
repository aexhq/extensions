set -euo pipefail
trap 'status=$?; (( status == 0 )) || printf "restricted network canary shell failed: status=%s\n" "$status" >&2' EXIT

connector_class=__CLASS__
denied=(__DENIED__)
controls=(__CONTROLS__)
gateway_host=__GATEWAY_HOST__
gateway_port=__GATEWAY_PORT__
require_gateway=__REQUIRE_GATEWAY__

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

gateway_health_status() {
  curl --disable --ipv4 --noproxy '*' --silent --output /dev/null \
    --connect-timeout 1.5 --max-time 3 --write-out '%{http_code}' \
    "http://$gateway_host:$gateway_port/healthz" 2>/dev/null || true
}

gateway_connect_status() {
  local capability=${1-}
  local -a auth=()
  if [[ -n $capability ]]; then
    auth=(--proxy-header "Proxy-Authorization: Bearer $capability")
  fi
  curl --disable --ipv4 --noproxy '' --silent --output /dev/null \
    --connect-timeout 1.5 --max-time 3 \
    --proxy "http://$gateway_host:$gateway_port" --proxytunnel \
    "${auth[@]}" --write-out '%{http_connect}' https://example.com/ 2>/dev/null || true
}

if timeout 3 getent ahostsv4 example.com >/dev/null 2>&1; then
  echo 'restricted connector DNS was not fail-closed' >&2
  exit 1
fi

direct_hosts=("${denied[@]}" "${controls[@]}")
if (( ! require_gateway )); then
  direct_hosts+=("$gateway_host")
fi
reachable_direct=$(probe_hosts '53 80 443 8443' "${direct_hosts[@]}")
if [[ -n $reachable_direct ]]; then
  echo "restricted connector accepted direct TCP: ${reachable_direct//$'\n'/,}" >&2
  exit 1
fi

if (( require_gateway )); then
  health=$(gateway_health_status)
  [[ $health == 200 ]] || { echo "allowlist gateway health was not reachable: ${health:-unreachable}" >&2; exit 1; }
  unauthenticated=$(gateway_connect_status)
  [[ $unauthenticated == 407 ]] || { echo "allowlist gateway accepted or misclassified missing auth: ${unauthenticated:-unreachable}" >&2; exit 1; }
  invalid=$(gateway_connect_status invalid-release-canary-capability)
  [[ $invalid == 403 ]] || { echo "allowlist gateway accepted or misclassified invalid auth: ${invalid:-unreachable}" >&2; exit 1; }
fi

printf 'restricted_network_canary=ok class=%s denied=%s controls=%s\n' "$connector_class" "${#denied[@]}" "${#controls[@]}"

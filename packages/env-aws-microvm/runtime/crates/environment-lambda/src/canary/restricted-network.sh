set -euo pipefail
trap 'status=$?; (( status == 0 )) || printf "restricted network canary shell failed: status=%s\n" "$status" >&2' EXIT

connector_class=__CLASS__
denied=(__DENIED__)
controls=(__CONTROLS__)
gateway_host=__GATEWAY_HOST__
gateway_port=__GATEWAY_PORT__
require_gateway=__REQUIRE_GATEWAY__

probe() {
  timeout 1.5 bash -c "exec 3<>/dev/tcp/$1/$2" >/dev/null 2>&1
}

probe_hosts() {
  local ports=$1 host port pid
  local -a pids=()
  shift
  for host in "$@"; do
    (
      for port in $ports; do
        if probe "$host" "$port"; then
          printf '%s\n' "$host"
          exit 0
        fi
      done
      exit 0
    ) &
    pids+=("$!")
    if (( ${#pids[@]} == 8 )); then
      for pid in "${pids[@]}"; do wait "$pid" || true; done
      pids=()
    fi
  done
  for pid in "${pids[@]}"; do wait "$pid" || true; done
}

gateway_status() {
  local request=$1 line
  exec 3<>"/dev/tcp/$gateway_host/$gateway_port" || return 1
  printf '%b' "$request" >&3
  IFS= read -r -t 3 line <&3 || return 1
  exec 3>&- 3<&-
  line=${line%$'\r'}
  [[ $line =~ ^HTTP/1\.1\ ([0-9]{3})\  ]] || return 1
  printf '%s\n' "${BASH_REMATCH[1]}"
}

if getent ahostsv4 example.com >/dev/null 2>&1; then
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
  health=$(gateway_status "GET /healthz HTTP/1.1\r\nHost: $gateway_host\r\nConnection: close\r\n\r\n" || true)
  [[ $health == 200 ]] || { echo "allowlist gateway health was not reachable: ${health:-unreachable}" >&2; exit 1; }
  unauthenticated=$(gateway_status 'CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nConnection: close\r\n\r\n' || true)
  [[ $unauthenticated == 407 ]] || { echo "allowlist gateway accepted or misclassified missing auth: ${unauthenticated:-unreachable}" >&2; exit 1; }
  invalid=$(gateway_status 'CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nProxy-Authorization: Bearer invalid-release-canary-capability\r\nConnection: close\r\n\r\n' || true)
  [[ $invalid == 403 ]] || { echo "allowlist gateway accepted or misclassified invalid auth: ${invalid:-unreachable}" >&2; exit 1; }
fi

printf 'restricted_network_canary=ok class=%s denied=%s controls=%s\n' "$connector_class" "${#denied[@]}" "${#controls[@]}"

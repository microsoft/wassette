#!/usr/bin/env bash
# Copyright (c) Microsoft Corporation.
# Licensed under the MIT license.

# Smoke-test the three terminal coding agents against Wassette, end to end.
#
# The assertion, for each client, in ONE session with NO restart:
#   1. start the client with Wassette configured as an MCP server
#   2. have it call load-component with a known component reference
#   3. have it call a tool exported by that component
#   4. PASS = step 3 was dispatched and returned; FAIL = it was not
#
# Everything is asserted against the client's own structured event stream and
# against a timestamped tap of the JSON-RPC wire, never against the model's
# prose. See docs/development/getting-started.md.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAP="$REPO_ROOT/scripts/mcp-client-tap.py"

COMPONENT="${WASSETTE_CLIENTS_COMPONENT:-oci://ghcr.io/microsoft/time-server-js:latest}"
TOOL="${WASSETTE_CLIENTS_TOOL:-microsoft_time-server-js_time_get-current-time}"
WASSETTE_BIN="${WASSETTE_BIN:-$REPO_ROOT/bin/wassette}"
COPILOT_TOKEN_FILE="${WASSETTE_CLIENTS_COPILOT_TOKEN_FILE:-}"
TIMEOUT="${WASSETTE_CLIENTS_TIMEOUT:-420}"

# Run dirs must not live under /tmp: codex refuses to create its PATH aliases
# when CODEX_HOME is a temporary directory and warns on every invocation.
RUN_ROOT="${WASSETTE_CLIENTS_RUN_DIR:-$HOME/.cache/wassette-mcp-clients/runs/$(date +%Y%m%d-%H%M%S)}"

NEGATIVE=0
CLIENTS=()

usage() {
    cat <<'USAGE'
Usage: scripts/test-mcp-clients.sh [--negative] [--run-dir DIR] [copilot|claude|codex ...]

  --negative      Deliberately broken case: instruct the agent NOT to call the
                  component's tool. A correct harness must report FAIL. This is
                  the harness's own self-test.
  --run-dir DIR   Where to put components, configs, logs and transcripts.

With no clients named, runs all three in descending-confidence order:
copilot (the #308 regression guard), claude (#111), codex (#259).

Exit: 0 all PASS, 1 some FAIL, 2 some BLOCKED, 3 some VOID.

VOID means a premise of the test did not hold (the component was already
present, the client reconnected, or the wire transcript was tampered with), so
the run proves nothing either way. It is deliberately not folded into FAIL.
USAGE
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --negative) NEGATIVE=1; shift ;;
        --run-dir) RUN_ROOT="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        copilot|claude|codex) CLIENTS+=("$1"); shift ;;
        *) echo "unknown argument: $1" >&2; usage >&2; exit 64 ;;
    esac
done
[[ ${#CLIENTS[@]} -eq 0 ]] && CLIENTS=(copilot claude codex)

# The prompt is one unambiguous instruction naming both calls. The tool name is
# given rather than discovered on purpose: the question under test is whether
# the client can dispatch a tool that appeared mid-session, not whether a model
# can guess its name.
PROMPT_PASS="Call the wassette load-component tool with path ${COMPONENT}. Then call the tool named ${TOOL} with no arguments. Report the value it returns."
PROMPT_NEGATIVE="Call the wassette load-component tool with path ${COMPONENT}. Then STOP. Do not call ${TOOL} or any other tool. Just describe in words what you would have called."
PROMPT="$PROMPT_PASS"
[[ $NEGATIVE -eq 1 ]] && PROMPT="$PROMPT_NEGATIVE"

declare -A RESULT REASON
BOLD=$'\033[1m'; RED=$'\033[31m'; GRN=$'\033[32m'; YEL=$'\033[33m'; DIM=$'\033[2m'; OFF=$'\033[0m'

log()  { printf '%s==>%s %s\n' "$BOLD" "$OFF" "$*"; }
info() { printf '    %s%s%s\n' "$DIM" "$*" "$OFF"; }

# --- workspace -------------------------------------------------------------

# A stale component directory is the one thing that would make this test pass
# for the wrong reason: the tool would already be present when the client
# started, and nothing dynamic would be under test. Every run gets an empty one.
prepare_client_dir() {
    local client="$1" d="$RUN_ROOT/$client"
    mkdir -p "$d"/{logs,cwd,shim,codexhome} "$d/xdg/wassette/components"
    cat > "$d/shim/wassette" <<EOF
#!/usr/bin/env bash
# Sits on PATH ahead of the real binary so the documented setup command stays
# literally "wassette run" while both directions of the wire get recorded.
#
# Isolation is done with the global --component-dir flag, passed HERE by the
# shim itself. The shim is load-bearing: setting this from the calling shell
# does not work, because codex launches MCP servers through its long-lived
# app-server daemon with a sanitised environment. The shim is our own process,
# so what it passes always arrives.
#
# Both levers are set, at the same directory, on purpose. A build carrying the
# upstream fix honours --component-dir for the startup restore as well as for
# writes. Released 0.6.0 does not: it takes the flag for writes but restores
# from the default directory, which XDG_DATA_HOME moves. Setting both means the
# harness is correct on either build instead of silently testing nothing on one
# of them, and check_premises catches it regardless by asserting the startup
# catalogue holds built-ins only.
exec env RUST_LOG="\${WASSETTE_CLIENTS_RUST_LOG:-info}" XDG_DATA_HOME="$d/xdg" "$WASSETTE_BIN" --component-dir "$d/xdg/wassette/components" "\$@" \\
  < <("$TAP" "$d/logs/client-to-server.jsonl") \\
  > >("$TAP" "$d/logs/server-to-client.jsonl") \\
  2> >(tee -a "$d/logs/server.stderr.log" >&2)
EOF
    chmod +x "$d/shim/wassette"
    echo "$d"
}

# --- assertions ------------------------------------------------------------

# Each of these reads the client's own machine-readable event stream and answers
# one question: was a tool call BY NAME dispatched, and did it RETURN a result?
# A model that merely says it called the tool produces no such event.
#
# All three parse with `fromjson?` rather than `inputs`, so a stray non-JSON line
# is skipped instead of aborting the whole filter. That is not hypothetical: the
# first Copilot run printed `copilot mcp add`'s human-readable confirmation ahead
# of the event stream, jq failed on line 1, the assertion came back vacuously
# false, and a clean PASS was reported as a FAIL.
assert_claude() {
    local f="$1"
    jq -e -n -R --arg t "$TOOL" '
      [inputs | fromjson?] as $ev
      | ([ $ev[] | select(.type=="assistant") | .message.content[]?
           | select(.type=="tool_use" and (.name == ("mcp__wassette__" + $t))) | .id ]) as $ids
      | ($ids | length > 0)
        and ([ $ev[] | select(.type=="user") | .message.content[]?
               | select(.type=="tool_result" and (.tool_use_id as $i | $ids | index($i)))
               | select(.is_error != true) ] | length > 0)
    ' < "$f" >/dev/null 2>&1
}

assert_codex() {
    local f="$1"
    jq -e -n -R --arg t "$TOOL" '
      [inputs | fromjson?]
      | [ .[] | select(.type=="item.completed") | .item
          | select(.type=="mcp_tool_call" and .tool==$t
                   and .status=="completed" and (.error==null)) ]
      | length > 0
    ' < "$f" >/dev/null 2>&1
}

# Copilot 1.0.81 names the MCP call in `tool.execution_start` as `mcpToolName`,
# with `toolName` the server-prefixed form, and reports the outcome in a separate
# `tool.execution_complete` correlated by `toolCallId`.
assert_copilot() {
    local f="$1"
    jq -e -n -R --arg t "$TOOL" '
      [inputs | fromjson?] as $ev
      | ([ $ev[] | select(.type=="tool.execution_start")
           | select(((.data.mcpToolName // "") == $t)
                    and ((.data.mcpServerName // "") == "wassette"))
           | .data.toolCallId ]) as $ids
      | ($ids | length > 0)
        and ([ $ev[] | select(.type=="tool.execution_complete")
               | select(.data.toolCallId as $i | $ids | index($i))
               | select(.data.success == true) ] | length > 0)
    ' < "$f" >/dev/null 2>&1
}

# --- premises --------------------------------------------------------------

# A verdict is only worth printing if the run actually tested what it claims to.
# Each of these checks one premise of the claim "a component loaded mid-session,
# in one session, with no restart". A violation makes the result VOID, which is
# reported separately from PASS and FAIL rather than being folded into either.
check_premises() {
    local d="$1" c2s="$1/logs/client-to-server.jsonl" s2c="$1/logs/server-to-client.jsonl"
    local v=()

    # The component must not have been present at startup.
    [[ -z "$(ls -A "$d/xdg/wassette/components" 2>/dev/null)" ]] &&
        v+=("component directory is empty, so the component never landed in this run's directory")

    # "One session, no restart" has to be asserted, not assumed. More than one
    # handshake means the client reconnected and could have picked the tool up
    # from a fresh startup list rather than from the notification.
    #
    # Both protocol eras count: pre-2026-07-28 clients open with `initialize`,
    # and 2026-07-28 clients with `server/discover`. Copilot CLI 1.0.81
    # negotiates the newer one, so checking only for `initialize` reports a
    # perfectly good session as having no handshake at all.
    local inits
    inits=$(awk -F'\t' '{print $2}' "$c2s" 2>/dev/null | jq -rc 'select(.method=="initialize" or .method=="server/discover")|.method' 2>/dev/null | wc -l)
    [[ "$inits" -ne 1 ]] && v+=("saw $inits session handshakes (initialize/server/discover), expected exactly 1: the no-restart premise does not hold")

    # The strongest form of the isolation premise, and the one that does not
    # depend on which lever set the component directory or on which build of
    # wassette is in use: the catalogue the client saw at startup must contain
    # the built-in tools and nothing else. If a component was restored, the
    # client had nothing to discover and the run says nothing about dynamic
    # loading.
    #
    # Matching the target tool by name is not enough. A component restored from
    # disk is currently exposed as `<unnamed>` rather than under its real name,
    # so a name check sails straight past the very case this is guarding.
    # Only the FIRST tools/list response counts. Every later one legitimately
    # contains the component's tool, that being the whole point of the test.
    local strays
    strays=$(awk -F'\t' '{print $2}' "$s2c" 2>/dev/null \
        | jq -c 'select(.result.tools)' 2>/dev/null | head -1 \
        | jq -r '.result.tools[]|(.name // "<no-name>")' 2>/dev/null \
        | grep -vxE 'load-component|unload-component|list-components|get-policy|search-components|reset-permission|(grant|revoke)-(storage|network|environment-variable)-permission' \
        | sort -u | tr '\n' ' ')
    [[ -n "${strays// /}" ]] &&
        v+=("startup tools/list already carried non-builtin tool(s) ${strays% }, so nothing was loaded dynamically")

    # The tapped transcript must be complete. A response with no matching request
    # means something wrote to the server's stdin behind the tap, which is not
    # hypothetical: a Codex run with shell access did exactly this.
    local orphans
    orphans=$(comm -13 \
        <(awk -F'\t' '{print $2}' "$c2s" 2>/dev/null | jq -rc 'select(.id!=null)|.id|tostring' 2>/dev/null | sort -u) \
        <(awk -F'\t' '{print $2}' "$s2c" 2>/dev/null | jq -rc 'select(.id!=null)|.id|tostring' 2>/dev/null | sort -u) | tr '\n' ' ')
    [[ -n "${orphans// /}" ]] &&
        v+=("wire transcript is contaminated: response id(s) ${orphans% } have no matching request, so something bypassed the tap")

    local IFS='; '; echo "${v[*]}"
}

# Cross-check the client's story against the wire. The event stream says the
# client believes it called the tool; the wire says the call reached Wassette and
# came back. Requiring both defeats a replayed or cached client event, and a call
# that a proxy or another server answered.
assert_wire() {
    local c2s="$1/logs/client-to-server.jsonl" s2c="$1/logs/server-to-client.jsonl"
    local id
    id=$(awk -F'\t' '{print $2}' "$c2s" 2>/dev/null \
         | jq -rc --arg t "$TOOL" 'select(.method=="tools/call" and .params.name==$t)|.id|tostring' 2>/dev/null | head -1)
    [[ -z "$id" ]] && return 1
    awk -F'\t' '{print $2}' "$s2c" 2>/dev/null \
      | jq -e --argjson i "$id" 'select((.id|tostring)==($i|tostring))
            | select(.error == null) | select(.result.isError != true)' >/dev/null 2>&1
}

# --- server-vs-client attribution -----------------------------------------

# The point of the wire tap: a FAIL is only a report, rather than an argument,
# if it can say which side broke. Notification sent but never followed by a
# tools/list means the client ignored it. Notification never sent means the
# fault is Wassette's and #111/#259 need rewriting.
attribute() {
    local d="$1" s2c="$1/logs/server-to-client.jsonl" c2s="$1/logs/client-to-server.jsonl"
    local n_notif=0 n_list_after=0 t_notif=""

    if [[ -s "$s2c" ]]; then
        n_notif=$(grep -c 'notifications/tools/list_changed' "$s2c" 2>/dev/null || echo 0)
        t_notif=$(grep -m1 'notifications/tools/list_changed' "$s2c" 2>/dev/null | cut -f1)
    fi
    if [[ -n "$t_notif" && -s "$c2s" ]]; then
        n_list_after=$(awk -F'\t' -v t="$t_notif" '$1 > t && /"method":"tools\/list"/ {n++} END{print n+0}' "$c2s")
    fi

    # An agent with a shell can call the tool without going through its client,
    # which is exactly what Codex does when its catalogue never refreshes. The
    # wire then shows a successful call that the client's own event stream knows
    # nothing about. Name it, so the FAIL is not mistaken for a harness error.
    if assert_wire "$d" && ! "assert_$(basename "$d")" "$d/logs/client.jsonl" 2>/dev/null; then
        echo "the tool WAS called successfully on the wire but the client's event stream has no record of it: the agent bypassed its own client (out-of-band call), so this is not a working tool catalogue"
        return
    fi

    if [[ "$n_notif" -eq 0 ]]; then
        echo "server: notifications/tools/list_changed was NEVER SENT -> Wassette-side fault"
    elif [[ "$n_list_after" -eq 0 ]]; then
        echo "server sent list_changed (${n_notif}x); client issued NO tools/list afterwards -> client-side fault"
    else
        echo "server sent list_changed (${n_notif}x); client re-listed ${n_list_after}x afterwards -> client honoured the notification"
    fi
}

# --- clients ---------------------------------------------------------------

# Auth preflight makes a REAL call with no MCP configured. Checking that a
# variable is merely set is what made #53's bootstrap report "ok" and then die
# at the first request.
copilot_token() {
    if [[ -n "${COPILOT_GITHUB_TOKEN:-}" ]]; then
        printf '%s' "$COPILOT_GITHUB_TOKEN"
    elif [[ -n "$COPILOT_TOKEN_FILE" && -r "$COPILOT_TOKEN_FILE" ]]; then
        printf '%s' "$(<"$COPILOT_TOKEN_FILE")"
    fi
}

preflight() {
    case "$1" in
        copilot)
            local token
            token="$(copilot_token)"
            if [[ -n "$token" ]]; then
                COPILOT_GITHUB_TOKEN="$token" timeout 120 copilot --allow-all-tools -p "Reply with exactly: ok"
            else
                timeout 120 copilot --allow-all-tools -p "Reply with exactly: ok"
            fi >/dev/null 2>"$RUN_ROOT/copilot.preflight.err" </dev/null \
                || { echo "copilot is not authenticated: $(head -1 "$RUN_ROOT/copilot.preflight.err")"; return 1; } ;;
        claude)
            timeout 120 claude -p "Reply with exactly: ok" --output-format json >/dev/null 2>"$RUN_ROOT/claude.preflight.err" </dev/null \
                || { echo "claude is not authenticated: $(head -1 "$RUN_ROOT/claude.preflight.err")"; return 1; } ;;
        codex)
            timeout 120 codex exec --skip-git-repo-check --json "Reply with exactly: ok" >/dev/null 2>"$RUN_ROOT/codex.preflight.err" </dev/null \
                || { echo "codex is not authenticated: $(head -1 "$RUN_ROOT/codex.preflight.err")"; return 1; } ;;
    esac
    return 0
}

run_copilot() {
    local d; d="$(prepare_client_dir copilot)"
    local backup="$d/mcp-config.json.bak" live="$HOME/.copilot/mcp-config.json"
    local token
    token="$(copilot_token)"
    [[ -f "$live" ]] && cp -a "$live" "$backup"
    restore_copilot() { if [[ -f "$backup" ]]; then cp -a "$backup" "$live"; else rm -f "$live"; fi; }
    trap restore_copilot RETURN

    (
      export PATH="$d/shim:$PATH"
      [[ -n "$token" ]] && export COPILOT_GITHUB_TOKEN="$token"
      cd "$d/cwd" || exit 1
      copilot mcp add wassette -- wassette run >"$d/logs/setup.log" 2>&1   # documented, unmodified
      timeout "$TIMEOUT" copilot --allow-all-tools --output-format json -p "$PROMPT"
    ) > "$d/logs/client.jsonl" 2> "$d/logs/client.err" </dev/null
    assert_copilot "$d/logs/client.jsonl"
}

run_claude() {
    local d; d="$(prepare_client_dir claude)"
    (
      export PATH="$d/shim:$PATH"
      cd "$d/cwd" || exit 1
      claude mcp add -- wassette wassette run >/dev/null            # documented, unmodified
      # --allowedTools scopes approval to this one server rather than disabling
      # permission checks wholesale.
      timeout "$TIMEOUT" claude -p "$PROMPT" --output-format stream-json --verbose \
          --allowedTools "mcp__wassette"
      claude mcp remove wassette >/dev/null 2>&1
    ) > "$d/logs/client.jsonl" 2> "$d/logs/client.err" </dev/null
    assert_claude "$d/logs/client.jsonl"
}

run_codex() {
    local d; d="$(prepare_client_dir codex)"
    cp -a "$HOME/.codex/auth.json" "$d/codexhome/" 2>/dev/null
    (
      export PATH="$d/shim:$PATH" CODEX_HOME="$d/codexhome"
      cd "$d/cwd" || exit 1
      codex mcp add wassette wassette run >/dev/null                # documented, unmodified
      # codex exec offers only approval policies 'on-request' and 'never', and
      # 'never' hard-fails EVERY MCP tool call. With no human to answer a
      # prompt, this flag is the only way a non-interactive codex can call an
      # MCP tool at all. The surrounding environment supplies the external
      # sandbox that the flag documents.
      timeout "$TIMEOUT" codex exec --skip-git-repo-check --json \
          --dangerously-bypass-approvals-and-sandbox "$PROMPT"
    ) > "$d/logs/client.jsonl" 2> "$d/logs/client.err" </dev/null
    assert_codex "$d/logs/client.jsonl"
}

# --- main ------------------------------------------------------------------

mkdir -p "$RUN_ROOT"
log "Wassette client smoke test"
info "wassette   $("$WASSETTE_BIN" --version 2>/dev/null | head -1 | cut -d' ' -f1)"
info "component  $COMPONENT"
info "tool       $TOOL"
info "run dir    $RUN_ROOT"
[[ $NEGATIVE -eq 1 ]] && info "mode       NEGATIVE CONTROL (a correct harness reports FAIL for every client)"
echo

for client in "${CLIENTS[@]}"; do
    log "$client"
    if ! reason="$(preflight "$client")"; then
        RESULT[$client]=BLOCKED; REASON[$client]="$reason"
        info "BLOCKED: $reason"; echo; continue
    fi
    info "auth ok (verified with a real call)"

    events_say_yes=0
    "run_$client" && events_say_yes=1
    d="$RUN_ROOT/$client"
    violations="$(check_premises "$d")"

    if [[ -n "$violations" ]]; then
        RESULT[$client]=VOID
        REASON[$client]="$violations"
    elif [[ $events_say_yes -eq 1 ]] && assert_wire "$d"; then
        RESULT[$client]=PASS
        REASON[$client]="$(attribute "$d")"
    elif [[ $events_say_yes -eq 1 ]]; then
        # The client reported a tool call the wire has no record of. That is a
        # statement about the harness or the client, not about Wassette.
        RESULT[$client]=VOID
        REASON[$client]="event stream reports the call but the wire has no successful tools/call for it; the two disagree"
    else
        RESULT[$client]=FAIL
        REASON[$client]="$(attribute "$d")"
    fi
    info "${RESULT[$client]}: ${REASON[$client]}"
    echo
done

log "Summary"
rc=0; blocked=0; void=0; passed=0; failed=0
for client in "${CLIENTS[@]}"; do
    r="${RESULT[$client]:-FAIL}"
    case "$r" in
        PASS)    c="$GRN"; passed=$((passed+1)) ;;
        FAIL)    c="$RED"; failed=$((failed+1)) ;;
        VOID)    c="$YEL"; void=1 ;;
        BLOCKED) c="$YEL"; blocked=1 ;;
    esac
    printf '  %-9s %s%-7s%s %s\n' "$client" "$c" "$r" "$OFF" "${REASON[$client]:-}"
done

echo
if [[ $NEGATIVE -eq 1 ]]; then
    # Inverted: the deliberately broken case is only meaningful if it comes back
    # red. A PASS here means the assertion is vacuously true and every green run
    # this harness has ever printed is worthless.
    if [[ $passed -gt 0 ]]; then
        printf '  %snegative control DEFEATED: %d client(s) reported PASS while the tool was never called%s\n' "$RED" "$passed" "$OFF"
        rc=1
    else
        printf '  %snegative control held: no client reported PASS%s\n' "$GRN" "$OFF"
        rc=0
    fi
else
    [[ $failed -gt 0 ]] && rc=1
    [[ $rc -eq 0 && $void -eq 1 ]] && rc=3
    [[ $rc -eq 0 && $blocked -eq 1 ]] && rc=2
fi

info "transcripts and wire taps under $RUN_ROOT"
exit $rc
